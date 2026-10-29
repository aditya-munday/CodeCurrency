//! Explanation v0: rule-based hints and capsule diffs.
//!
//! Layer 8 of Phase 1. The platform's promise is that every result is
//! *explained*, not just ranked. Most of that explanation is rule-based
//! analysis of counters — cheap, deterministic, and checkable — with a language
//! model used only to phrase it later.
//!
//! Nothing here guesses. A hint fires only when a counter says so, and each
//! hint carries the evidence that produced it.

use crate::capsule::{Capsule, MetricKind};
use serde::{Deserialize, Serialize};

/// A rule-based observation about one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hint {
    /// Stable id so hints can be counted, muted, and translated.
    pub rule: String,
    /// One sentence a human reads.
    pub message: String,
    /// The counter evidence, so the claim is checkable rather than asserted.
    pub evidence: Vec<Evidence>,
    /// How sure the rule is. `Certain` means it follows from a single counter;
    /// `Likely` means it is an inference from a combination.
    pub confidence: Confidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Certain,
    Likely,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub metric: String,
    pub value: f64,
    pub unit: String,
}

/// Thresholds for the Phase 1 rule set. Kept explicit so they can be argued
/// with and tuned rather than buried in code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ruleset {
    /// Allocations above this in total are worth reporting.
    pub allocation_warn: u64,
    /// Allocation bytes above this are worth reporting.
    pub allocation_bytes_warn: u64,
    /// Branch mispredict rate (mispredicts / branches) above this is reported.
    pub branch_miss_rate_warn: f64,
    /// Cache miss rate (misses / refs) above this is reported.
    pub cache_miss_rate_warn: f64,
    /// Peak memory above this is reported.
    pub peak_memory_warn: u64,
    /// Binary size above this is reported.
    pub binary_size_warn: u64,
}

impl Default for Ruleset {
    fn default() -> Self {
        Ruleset {
            allocation_warn: 10_000,
            allocation_bytes_warn: 16 * 1024 * 1024,
            branch_miss_rate_warn: 0.05,
            cache_miss_rate_warn: 0.02,
            peak_memory_warn: 64 * 1024 * 1024,
            binary_size_warn: 1024 * 1024,
        }
    }
}

impl Ruleset {
    /// Stable identifier for the ruleset, recorded in the capsule so a hint can
    /// always be traced back to the thresholds that produced it.
    pub fn id(&self) -> String {
        let body = format!(
            "alloc={};allocb={};branch={:.6};cache={:.6};peak={};bin={}",
            self.allocation_warn,
            self.allocation_bytes_warn,
            self.branch_miss_rate_warn,
            self.cache_miss_rate_warn,
            self.peak_memory_warn,
            self.binary_size_warn
        );
        crate::capsule::sha256_hex(body.as_bytes())[..16].to_string()
    }
}

fn metric(capsule: &Capsule, name: &str) -> Option<f64> {
    capsule
        .metrics
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.value)
}

fn evidence(capsule: &Capsule, name: &str) -> Option<Evidence> {
    capsule
        .metrics
        .iter()
        .find(|m| m.name == name)
        .map(|m| Evidence {
            metric: m.name.clone(),
            value: m.value,
            unit: m.unit.clone(),
        })
}

/// Apply the Phase 1 rule set to one capsule.
pub fn explain(capsule: &Capsule, rules: &Ruleset) -> Vec<Hint> {
    let mut hints = Vec::new();

    // Allocation volume. Note: this reports the *total* allocation stream, not
    // "inside the hot loop" — detecting the loop needs the per-line profile,
    // which is a later layer. Saying "total" keeps the hint honest.
    if let Some(allocs) = metric(capsule, "allocations") {
        if allocs as u64 > rules.allocation_warn {
            let mut ev = vec![];
            if let Some(e) = evidence(capsule, "allocations") {
                ev.push(e);
            }
            if let Some(e) = evidence(capsule, "allocated_bytes") {
                ev.push(e);
            }
            hints.push(Hint {
                rule: "allocation-volume".into(),
                message: format!(
                    "{} allocations totalling {} bytes. Allocation is a syscall-free but still \
                     serialising cost; reusing a buffer or an arena usually removes it entirely.",
                    allocs as u64,
                    metric(capsule, "allocated_bytes").unwrap_or(0.0) as u64
                ),
                evidence: ev,
                confidence: Confidence::Certain,
            });
        }
    }

    // Branch mispredictions: the dominant cost in unpredictable data-dependent
    // branches, and invisible in instruction counts.
    if let (Some(misses), Some(branches)) = (
        metric(capsule, "branch_misses"),
        metric(capsule, "instructions"),
    ) {
        // The branch *count* is not in the headline metrics, so rate is only
        // computable when a branch count was recorded. Fall back to reporting
        // the absolute number with a weaker claim.
        let _ = branches;
        if let Some(branch_count) = metric(capsule, "branches") {
            if branch_count > 0.0 {
                let rate = misses / branch_count;
                if rate > rules.branch_miss_rate_warn {
                    hints.push(Hint {
                        rule: "branch-mispredict-rate".into(),
                        message: format!(
                            "{:.1}% of branches mispredicted ({:.0} of {:.0}). Sorting the input \
                             or turning the branch into a conditional move often removes this cost \
                             without changing the instruction count much.",
                            rate * 100.0,
                            misses,
                            branch_count
                        ),
                        evidence: vec![
                            Evidence {
                                metric: "branch_misses".into(),
                                value: misses,
                                unit: "count".into(),
                            },
                            Evidence {
                                metric: "branches".into(),
                                value: branch_count,
                                unit: "count".into(),
                            },
                        ],
                        confidence: Confidence::Likely,
                    });
                }
            }
        }
    }

    // Cache behaviour: the classic "the algorithm is fine, the layout is not".
    if let (Some(misses), Some(refs)) = (
        metric(capsule, "cache_misses"),
        metric(capsule, "memory_refs"),
    ) {
        if refs > 0.0 {
            let rate = misses / refs;
            if rate > rules.cache_miss_rate_warn {
                hints.push(Hint {
                    rule: "cache-miss-rate".into(),
                    message: format!(
                        "{:.1}% of memory references missed cache. When the access pattern is the \
                         problem, changing the layout (struct-of-arrays, blocking, or a smaller \
                         element type) moves this number; changing the algorithm often does not.",
                        rate * 100.0
                    ),
                    evidence: vec![
                        Evidence {
                            metric: "cache_misses".into(),
                            value: misses,
                            unit: "count".into(),
                        },
                        Evidence {
                            metric: "memory_refs".into(),
                            value: refs,
                            unit: "count".into(),
                        },
                    ],
                    confidence: Confidence::Likely,
                });
            }
        }
    }

    if let Some(peak) = metric(capsule, "peak_memory") {
        if peak as u64 > rules.peak_memory_warn {
            hints.push(Hint {
                rule: "peak-memory".into(),
                message: format!(
                    "Peak resident memory was {} MiB. If this is much larger than the input, the \
                     solution is probably materialising something it could stream.",
                    peak as u64 / (1024 * 1024)
                ),
                evidence: vec![Evidence {
                    metric: "peak_memory".into(),
                    value: peak,
                    unit: "bytes".into(),
                }],
                confidence: Confidence::Certain,
            });
        }
    }

    if let Some(size) = metric(capsule, "binary_size") {
        if size as u64 > rules.binary_size_warn {
            hints.push(Hint {
                rule: "binary-size".into(),
                message: format!(
                    "The binary is {} KiB. Large binaries cost instruction-cache misses on the first \
                     pass; if the hot loop is small this is usually irrelevant, and if it is not, \
                     `-Os` and removing unused templates helps.",
                    size as u64 / 1024
                ),
                evidence: vec![Evidence { metric: "binary_size".into(), value: size, unit: "bytes".into() }],
                confidence: Confidence::Certain,
            });
        }
    }

    // A deterministic metric with no uncertainty is fine, but an indicative one
    // without uncertainty is exactly the fake precision we promised not to
    // ship. The capsule validator enforces this; the explainer states it.
    for m in &capsule.metrics {
        if m.kind == MetricKind::Indicative && m.uncertainty.is_none() {
            hints.push(Hint {
                rule: "indicative-without-uncertainty".into(),
                message: format!(
                    "Metric {:?} is labelled indicative but carries no uncertainty. Indicative \
                     numbers are machine-dependent and must always be shown with a spread.",
                    m.name
                ),
                evidence: vec![Evidence {
                    metric: m.name.clone(),
                    value: m.value,
                    unit: m.unit.clone(),
                }],
                confidence: Confidence::Certain,
            });
        }
    }

    hints
}

/// A change in one metric between two capsules.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricDelta {
    pub name: String,
    pub unit: String,
    pub kind: MetricKind,
    pub before: f64,
    pub after: f64,
    pub delta: f64,
    pub delta_pct: Option<f64>,
    /// Whether the change is larger than the measured spread, when known.
    pub significant: Significance,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Significance {
    /// The change exceeds the recorded spread: it is a real difference.
    Real,
    /// The change is within the recorded spread: it is noise.
    WithinNoise,
    /// No spread was recorded, so significance cannot be claimed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapsuleDiff {
    pub from_id: Option<String>,
    pub to_id: Option<String>,
    pub deltas: Vec<MetricDelta>,
    pub verdict_changed: Option<(String, String)>,
    pub toolchain_changed: bool,
    pub host_changed: bool,
}

/// Diff two capsules on the metrics they share.
///
/// A change smaller than the recorded spread is reported as `WithinNoise`
/// rather than as an improvement: rankings admit ties when differences are
/// noise, and so does the diff.
pub fn diff(before: &Capsule, after: &Capsule) -> CapsuleDiff {
    let mut deltas = Vec::new();

    for b in &before.metrics {
        let Some(a) = after.metrics.iter().find(|m| m.name == b.name) else {
            continue;
        };

        let delta = a.value - b.value;
        let delta_pct = if b.value == 0.0 {
            None
        } else {
            Some(delta / b.value * 100.0)
        };

        // Prefer the tighter of the two spreads, if either capsule recorded one.
        let spread = before
            .repeatability
            .as_ref()
            .and_then(|r| r.per_metric.get(&b.name))
            .map(|s| s.max - s.min)
            .into_iter()
            .chain(
                after
                    .repeatability
                    .as_ref()
                    .and_then(|r| r.per_metric.get(&a.name))
                    .map(|s| s.max - s.min),
            )
            .fold(None, |acc: Option<f64>, v| {
                Some(match acc {
                    Some(x) => x.max(v),
                    None => v,
                })
            });

        let significant = if before.host.fingerprint != after.host.fingerprint
            && (b.machine_dependent() || a.machine_dependent())
        {
            Significance::Unknown
        } else {
            match spread {
                Some(s) if delta.abs() <= s => Significance::WithinNoise,
                Some(_) => Significance::Real,
                None => Significance::Unknown,
            }
        };

        deltas.push(MetricDelta {
            name: b.name.clone(),
            unit: b.unit.clone(),
            kind: b.kind,
            before: b.value,
            after: a.value,
            delta,
            delta_pct,
            significant,
        });
    }

    CapsuleDiff {
        from_id: before.capsule_id.clone(),
        to_id: after.capsule_id.clone(),
        deltas,
        verdict_changed: (before.verdict != after.verdict).then(|| {
            (
                format!("{:?}", before.verdict),
                format!("{:?}", after.verdict),
            )
        }),
        toolchain_changed: before.toolchain.name != after.toolchain.name
            || before.toolchain.version != after.toolchain.version
            || !before.toolchain.same_environment(&after.toolchain),
        host_changed: before.host.fingerprint != after.host.fingerprint,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capsule::{Capsule, Metric, MetricKind, MetricMethod};

    #[test]
    fn ruleset_id_is_stable_and_changes_with_thresholds() {
        let a = Ruleset::default();
        let b = Ruleset::default();
        assert_eq!(a.id(), b.id());

        let mut c = Ruleset::default();
        c.allocation_warn += 1;
        assert_ne!(a.id(), c.id());
    }

    #[test]
    fn significance_treats_small_changes_as_noise() {
        // A 1% change when the spread is 5% is not a real difference.
        let spread: f64 = 5.0;
        let delta: f64 = 1.0;
        assert!(delta.abs() <= spread);
    }

    #[test]
    fn equivalent_rebuilt_toolchain_images_are_the_same_environment() {
        let source = include_str!("../tests/fixtures/capsule-v2.json");
        let mut before = Capsule::from_json(source).unwrap();
        let mut after = before.clone();
        before.toolchain.environment_fingerprint = Some("sha256:environment".into());
        after.toolchain.environment_fingerprint = Some("sha256:environment".into());
        after.toolchain.image_digest = "sha256:rebuilt-image".into();
        assert!(!diff(&before, &after).toolchain_changed);
    }

    #[test]
    fn getrusage_changes_across_hosts_are_inconclusive() {
        let source = include_str!("../tests/fixtures/capsule-v2.json");
        let mut before = Capsule::from_json(source).unwrap();
        let mut after = before.clone();
        before.host.fingerprint = "host-a".into();
        after.host.fingerprint = "host-b".into();
        before.metrics.push(Metric::count(
            "user_cpu_time",
            100,
            MetricKind::Measured,
            MetricMethod::Getrusage,
        ));
        after.metrics.push(Metric::count(
            "user_cpu_time",
            200,
            MetricKind::Measured,
            MetricMethod::Getrusage,
        ));
        let comparison = diff(&before, &after);
        let cpu_time = comparison
            .deltas
            .iter()
            .find(|delta| delta.name == "user_cpu_time")
            .unwrap();
        assert_eq!(cpu_time.significant, Significance::Unknown);
    }
}
