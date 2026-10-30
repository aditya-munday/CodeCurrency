//! Deterministic measurement: the trust layer.
//!
//! Layer 1 of Phase 1. The ranking signal is the set of metrics that do not
//! depend on which machine ran them: instruction counts, simulated cache and
//! branch misses, allocations, peak memory, and binary size. Wall-clock time is
//! recorded but labelled `indicative` and is never used for ranking.
//!
//! Primary method is `valgrind --tool=cachegrind`, which models the cache
//! deterministically, so two different machines produce the same numbers. When
//! a real hardware counter is available (`perf stat`), it is recorded
//! separately and labelled `measured`; it never replaces the simulated number.

use crate::capsule::{Metric, MetricKind, MetricMethod};
use serde::{Deserialize, Serialize};

/// Raw output of one cachegrind run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CachegrindCounts {
    pub instruction_refs: u64,
    pub data_refs: u64,
    pub data_reads: u64,
    pub data_writes: u64,
    pub instruction_cache_misses: u64,
    pub data_cache_misses: u64,
    pub branch_instructions: u64,
    pub branch_misses: u64,
}

impl CachegrindCounts {
    /// Total cache misses (instruction + data) as a single number.
    pub fn total_cache_misses(&self) -> u64 {
        self.instruction_cache_misses + self.data_cache_misses
    }

    /// Instructions retired, as modelled. This is the headline deterministic
    /// metric.
    pub fn instructions(&self) -> u64 {
        self.instruction_refs
    }

    pub fn to_metrics(&self) -> Vec<Metric> {
        let mut metrics = vec![
            Metric::count(
                "instructions",
                self.instructions(),
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "cache_misses",
                self.total_cache_misses(),
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "branch_misses",
                self.branch_misses,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "instruction_cache_misses",
                self.instruction_cache_misses,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "data_cache_misses",
                self.data_cache_misses,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "data_refs",
                self.data_refs,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "data_reads",
                self.data_reads,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "data_writes",
                self.data_writes,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "branch_instructions",
                self.branch_instructions,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
        ];
        let refs = self.data_refs.saturating_add(self.instruction_refs);
        if refs > 0 {
            metrics.push(Metric {
                name: "cache_miss_rate".to_string(),
                value: self.total_cache_misses() as f64 / refs as f64,
                unit: "ratio".to_string(),
                kind: MetricKind::Simulated,
                method: MetricMethod::Cachegrind,
                uncertainty: None,
            });
        }
        if self.branch_instructions > 0 {
            metrics.push(Metric {
                name: "branch_miss_rate".to_string(),
                value: self.branch_misses as f64 / self.branch_instructions as f64,
                unit: "ratio".to_string(),
                kind: MetricKind::Simulated,
                method: MetricMethod::Cachegrind,
                uncertainty: None,
            });
        }
        metrics
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("no cachegrind summary found in output")]
    MissingSummary,
    #[error("cachegrind summary is missing required counter: {0}")]
    IncompleteSummary(String),
    #[error("cachegrind summary contains a duplicate counter: {0}")]
    DuplicateCounter(String),
    #[error("cachegrind counter overflow while summing traced processes: {0}")]
    CounterOverflow(String),
    #[error("malformed cachegrind line: {0}")]
    Malformed(String),
    #[error("cachegrind reported an error: {0}")]
    Cachegrind(String),
}

/// Parse the summary block that `cachegrind` prints to stderr.
///
/// The format is a series of `name: value (rate%)` lines. We match on the
/// leading name so extra trailing detail (percentages, per-instruction rates)
/// does not break parsing.
pub fn parse_cachegrind_summary(stderr: &str) -> Result<CachegrindCounts, ParseError> {
    if stderr.contains("==error calling PR_SET_PTRACER") || stderr.contains("valgrind: failed") {
        return Err(ParseError::Cachegrind(
            stderr.lines().next().unwrap_or("").to_string(),
        ));
    }

    let mut total = CachegrindCounts::default();
    let mut current = CachegrindCounts::default();
    let mut seen = 0u16;

    for line in stderr.lines() {
        let line = line.trim();
        // Strip the `==pid== ` prefix valgrind adds.
        let line = match line.find("== ") {
            Some(idx) if line.starts_with("==") => line[idx + 3..].trim(),
            _ => line,
        };

        let (name, rest) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        // Cachegrind pads names with alignment spaces (`I   refs`), so collapse
        // runs of whitespace before matching.
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let value: u64 = match rest.trim().split_whitespace().next() {
            Some(tok) => match tok.replace(',', "").parse() {
                Ok(v) => v,
                Err(_) => continue,
            },
            None => continue,
        };

        match name.as_str() {
            "I refs" => {
                if seen != 0 {
                    require_complete_summary(seen)?;
                    add_cachegrind_counts(&mut total, &current)?;
                    current = CachegrindCounts::default();
                    seen = 0;
                }
                set_cachegrind_counter(
                    &mut current.instruction_refs,
                    value,
                    &mut seen,
                    1 << 0,
                    "I refs",
                )?;
            }
            "D refs" => {
                set_cachegrind_counter(&mut current.data_refs, value, &mut seen, 1 << 1, "D refs")?;
                let breakdown = rest
                    .split_once('(')
                    .map(|(_, text)| text.split(')').next().unwrap_or(text))
                    .unwrap_or("");
                let tokens: Vec<&str> = breakdown.split_whitespace().collect();
                for pair in tokens.windows(2) {
                    let parsed = pair[0].replace(',', "").parse::<u64>().ok();
                    match (pair[1], parsed) {
                        ("rd", Some(reads)) => set_cachegrind_counter(
                            &mut current.data_reads,
                            reads,
                            &mut seen,
                            1 << 2,
                            "data reads",
                        )?,
                        ("wr", Some(writes)) => set_cachegrind_counter(
                            &mut current.data_writes,
                            writes,
                            &mut seen,
                            1 << 3,
                            "data writes",
                        )?,
                        _ => {}
                    }
                }
            }
            "D1 misses" => set_cachegrind_counter(
                &mut current.data_cache_misses,
                value,
                &mut seen,
                1 << 4,
                "D1 misses",
            )?,
            "I1 misses" => set_cachegrind_counter(
                &mut current.instruction_cache_misses,
                value,
                &mut seen,
                1 << 5,
                "I1 misses",
            )?,
            "Dr" => set_cachegrind_counter(
                &mut current.data_reads,
                value,
                &mut seen,
                1 << 2,
                "data reads",
            )?,
            "Dw" => set_cachegrind_counter(
                &mut current.data_writes,
                value,
                &mut seen,
                1 << 3,
                "data writes",
            )?,
            "Branches" => set_cachegrind_counter(
                &mut current.branch_instructions,
                value,
                &mut seen,
                1 << 6,
                "Branches",
            )?,
            "Mispredicts" => set_cachegrind_counter(
                &mut current.branch_misses,
                value,
                &mut seen,
                1 << 7,
                "Mispredicts",
            )?,
            _ => continue,
        }
    }

    if seen == 0 {
        return Err(ParseError::MissingSummary);
    }
    require_complete_summary(seen)?;
    add_cachegrind_counts(&mut total, &current)?;
    total
        .instruction_cache_misses
        .checked_add(total.data_cache_misses)
        .ok_or_else(|| ParseError::CounterOverflow("total cache misses".to_string()))?;
    total
        .instruction_refs
        .checked_add(total.data_refs)
        .ok_or_else(|| ParseError::CounterOverflow("total memory references".to_string()))?;
    Ok(total)
}

fn set_cachegrind_counter(
    slot: &mut u64,
    value: u64,
    seen: &mut u16,
    bit: u16,
    name: &str,
) -> Result<(), ParseError> {
    if *seen & bit != 0 {
        return Err(ParseError::DuplicateCounter(name.to_string()));
    }
    *slot = value;
    *seen |= bit;
    Ok(())
}

fn require_complete_summary(seen: u16) -> Result<(), ParseError> {
    for (bit, name) in [
        (0, "I refs"),
        (1, "D refs"),
        (2, "data reads"),
        (3, "data writes"),
        (4, "D1 misses"),
        (5, "I1 misses"),
        (6, "Branches"),
        (7, "Mispredicts"),
    ] {
        if seen & (1 << bit) == 0 {
            return Err(ParseError::IncompleteSummary(name.to_string()));
        }
    }
    Ok(())
}

fn add_cachegrind_counts(
    total: &mut CachegrindCounts,
    current: &CachegrindCounts,
) -> Result<(), ParseError> {
    let add = |target: &mut u64, value: u64, name: &str| {
        *target = target
            .checked_add(value)
            .ok_or_else(|| ParseError::CounterOverflow(name.to_string()))?;
        Ok::<(), ParseError>(())
    };
    add(
        &mut total.instruction_refs,
        current.instruction_refs,
        "I refs",
    )?;
    add(&mut total.data_refs, current.data_refs, "D refs")?;
    add(&mut total.data_reads, current.data_reads, "data reads")?;
    add(&mut total.data_writes, current.data_writes, "data writes")?;
    add(
        &mut total.instruction_cache_misses,
        current.instruction_cache_misses,
        "I1 misses",
    )?;
    add(
        &mut total.data_cache_misses,
        current.data_cache_misses,
        "D1 misses",
    )?;
    add(
        &mut total.branch_instructions,
        current.branch_instructions,
        "Branches",
    )?;
    add(
        &mut total.branch_misses,
        current.branch_misses,
        "Mispredicts",
    )?;
    Ok(())
}

/// Parse `perf stat` output for the hardware counters we care about.
///
/// Returns `None` when no counter could be read, which is the honest answer on
/// a machine where `perf_event_paranoid` forbids access.
pub fn parse_perf_stat(stderr: &str) -> Option<(u64, u64, u64)> {
    let mut instructions = None;
    let mut cache_misses = None;
    let mut branch_misses = None;

    for line in stderr.lines() {
        let line = line.trim();
        let (value, name) = match line.split_once(' ') {
            Some(pair) => pair,
            None => continue,
        };
        let value: u64 = match value.replace(',', "").parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let name = name.trim();
        if name.starts_with("instructions") {
            instructions = Some(value);
        } else if name.starts_with("cache-misses") {
            cache_misses = Some(value);
        } else if name.starts_with("branch-misses") {
            branch_misses = Some(value);
        }
    }

    match (instructions, cache_misses, branch_misses) {
        (Some(i), Some(c), Some(b)) => Some((i, c, b)),
        _ => None,
    }
}

/// Whether the host permits reading hardware performance counters.
pub fn perf_available(perf_event_paranoid: Option<i32>) -> bool {
    matches!(perf_event_paranoid, Some(v) if v <= 2)
}

/// Read `/proc/sys/kernel/perf_event_paranoid` on the host.
pub fn read_perf_event_paranoid() -> Option<i32> {
    std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
==12345== I   refs:      1,234,567
==12345== I1  misses:        1,024
==12345== D   refs:        456,789  (300,000 rd + 156,789 wr)
==12345== D1  misses:        2,048
==12345== Branches:         50,000  (40,000 cond + 10,000 ind)
==12345== Mispredicts:       1,500
"#;

    #[test]
    fn parses_summary_block() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        assert_eq!(c.instruction_refs, 1_234_567);
        assert_eq!(c.instruction_cache_misses, 1_024);
        assert_eq!(c.data_refs, 456_789);
        assert_eq!(c.data_reads, 300_000);
        assert_eq!(c.data_writes, 156_789);
        assert_eq!(c.data_cache_misses, 2_048);
        assert_eq!(c.branch_instructions, 50_000);
        assert_eq!(c.branch_misses, 1_500);
    }

    #[test]
    fn totals_are_sums() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        assert_eq!(c.total_cache_misses(), 1_024 + 2_048);
        assert_eq!(c.instructions(), 1_234_567);
    }

    #[test]
    fn metrics_are_labelled_simulated() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        for m in c.to_metrics() {
            assert_eq!(m.kind, MetricKind::Simulated);
            assert_eq!(m.method, MetricMethod::Cachegrind);
        }
        let metrics = c.to_metrics();
        for name in [
            "instruction_cache_misses",
            "data_cache_misses",
            "data_refs",
            "data_reads",
            "data_writes",
            "branch_instructions",
            "cache_miss_rate",
            "branch_miss_rate",
        ] {
            assert!(metrics.iter().any(|metric| metric.name == name));
        }
        assert_eq!(
            metrics
                .iter()
                .find(|metric| metric.name == "branch_miss_rate")
                .unwrap()
                .value,
            0.03
        );
    }

    #[test]
    fn missing_summary_is_an_error_not_a_zero() {
        let err = parse_cachegrind_summary("nothing useful here").unwrap_err();
        assert!(matches!(err, ParseError::MissingSummary));
    }

    #[test]
    fn incomplete_summary_does_not_turn_missing_counters_into_zero() {
        let err = parse_cachegrind_summary("==12345== I refs: 1,234,567\n").unwrap_err();
        assert!(matches!(
            err,
            ParseError::IncompleteSummary(name) if name == "D refs"
        ));
    }

    #[test]
    fn traced_process_summaries_are_accumulated() {
        let first = SAMPLE.replace("12345", "100");
        let second = SAMPLE.replace("12345", "101");
        let counts = parse_cachegrind_summary(&format!("{first}{second}")).unwrap();
        assert_eq!(counts.instruction_refs, 2 * 1_234_567);
        assert_eq!(counts.data_refs, 2 * 456_789);
        assert_eq!(counts.data_reads, 2 * 300_000);
        assert_eq!(counts.data_writes, 2 * 156_789);
        assert_eq!(counts.total_cache_misses(), 2 * (1_024 + 2_048));
        assert_eq!(counts.branch_instructions, 100_000);
        assert_eq!(counts.branch_misses, 3_000);
    }

    #[test]
    fn incomplete_traced_process_is_rejected_before_the_next_summary() {
        let text = format!(
            "==100== I refs: 1\n==100== D refs: 2 (1 rd + 1 wr)\n{}",
            SAMPLE.replace("12345", "101")
        );
        let err = parse_cachegrind_summary(&text).unwrap_err();
        assert!(matches!(
            err,
            ParseError::IncompleteSummary(name) if name == "D1 misses"
        ));
    }

    #[test]
    fn repeated_counter_lines_in_one_process_are_rejected() {
        let text = SAMPLE.replace(
            "I1  misses:        1,024",
            "I1  misses:        1,024\n==12345== I1  misses:        1,024",
        );
        let err = parse_cachegrind_summary(&text).unwrap_err();
        assert!(matches!(err, ParseError::DuplicateCounter(name) if name == "I1 misses"));
    }

    #[test]
    fn traced_counter_sum_overflow_is_rejected() {
        let first = SAMPLE.replace("1,234,567", "18446744073709551615");
        let second = SAMPLE.replace("12345", "101");
        let err = parse_cachegrind_summary(&format!("{first}{second}")).unwrap_err();
        assert!(matches!(err, ParseError::CounterOverflow(name) if name == "I refs"));
    }

    #[test]
    fn complete_zero_counters_are_valid_measurements() {
        let text = "I refs: 0\nI1 misses: 0\nD refs: 0 (0 rd + 0 wr)\nD1 misses: 0\nBranches: 0 (0 cond + 0 ind)\nMispredicts: 0\n";
        let counts = parse_cachegrind_summary(text).unwrap();
        assert_eq!(counts, CachegrindCounts::default());
    }

    #[test]
    fn parses_perf_stat() {
        let text = "      1,234,567      instructions\n         12,345      cache-misses\n          1,234      branch-misses\n";
        let (i, c, b) = parse_perf_stat(text).unwrap();
        assert_eq!(i, 1_234_567);
        assert_eq!(c, 12_345);
        assert_eq!(b, 1_234);
    }

    #[test]
    fn partial_perf_stat_does_not_fabricate_missing_counters() {
        assert_eq!(
            parse_perf_stat("1,234 instructions\n12 cache-misses\n"),
            None
        );
    }

    #[test]
    fn perf_availability_follows_paranoid_level() {
        assert!(perf_available(Some(0)));
        assert!(perf_available(Some(2)));
        assert!(!perf_available(Some(3)));
        assert!(!perf_available(None));
    }
}
