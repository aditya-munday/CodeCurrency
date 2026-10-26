//! `ccx replay` — re-run a capsule and check whether it reproduces.
//!
//! This is the command the whole platform is built around. If a capsule cannot
//! be replayed to the same deterministic answer, the platform's claim is false,
//! and this command is where that would show up.
//!
//! Deterministic metrics must match exactly. Indicative metrics (wall time,
//! peak memory) are machine-dependent, so they are compared against the
//! recorded interval, not against a point value.

use crate::cmd::{self, Fixable};
use crate::context::{host_info, policy_for, Solution};
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::{Capsule, MetricKind, Verdict};
use ccx_core::engine::Measurement;
use serde::Serialize;

/// One metric's comparison between the recorded capsule and the replay.
#[derive(Debug, Clone, Serialize)]
pub struct MetricComparison {
    pub name: String,
    pub kind: MetricKind,
    pub recorded: f64,
    pub replayed: f64,
    pub delta: f64,
    pub matches: bool,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReplayReport {
    pub capsule_id: String,
    pub reproduced: bool,
    pub deterministic_match: bool,
    pub indicative_match: bool,
    pub comparisons: Vec<MetricComparison>,
    /// Environment differences that make exact reproduction impossible or
    /// expected, stated rather than hidden.
    pub environment_notes: Vec<String>,
}

/// Compare a recorded capsule against a fresh measurement.
///
/// A deterministic metric must match exactly. An indicative metric is compared
/// against its recorded interval; if it falls outside, the replay is reported
/// as not reproduced but the reason is attributed to the machine, not the
/// solution.
pub fn compare(recorded: &Capsule, replay: &Measurement, replay_host: &str) -> ReplayReport {
    let mut comparisons = Vec::new();
    let mut deterministic_match = true;
    let mut indicative_match = true;
    let mut environment_notes = Vec::new();

    if recorded.host.fingerprint != replay_host {
        environment_notes.push(format!(
            "recorded on a different machine ({} vs {}); indicative metrics are not comparable",
            &recorded.host.fingerprint[..12.min(recorded.host.fingerprint.len())],
            &replay_host[..12.min(replay_host.len())]
        ));
    }

    let replay_metrics = replay.to_metrics();

    for rec in &recorded.metrics {
        let Some(now) = replay_metrics.iter().find(|m| m.name == rec.name) else {
            comparisons.push(MetricComparison {
                name: rec.name.clone(),
                kind: rec.kind,
                recorded: rec.value,
                replayed: f64::NAN,
                delta: f64::NAN,
                matches: false,
                note: Some("metric is absent from the replay".into()),
            });
            deterministic_match = false;
            continue;
        };

        let delta = now.value - rec.value;
        let (matches, note) = match rec.kind {
            // Deterministic: an exact match is required, no tolerance.
            MetricKind::Simulated => {
                let ok = (delta.abs() < 0.5) as bool;
                (ok, (!ok).then(|| "simulated counter changed between runs".into()))
            }
            MetricKind::Measured => {
                let ok = (delta.abs() < 0.5) as bool;
                (ok, (!ok).then(|| "measured counter changed between runs".into()))
            }
            // Indicative: check against the recorded interval when there is one.
            MetricKind::Indicative => match &rec.uncertainty {
                Some(u) => {
                    let within = (now.value - rec.value).abs() <= u.value.max(1.0);
                    (
                        within,
                        (!within).then(|| {
                            format!(
                                "outside the recorded interval (+/- {:.3e}); machine-dependent",
                                u.value
                            )
                        }),
                    )
                }
                None => (
                    true,
                    Some("no recorded interval; not comparable, not counted as a failure".into()),
                ),
            },
        };

        match rec.kind {
            MetricKind::Indicative => indicative_match &= matches,
            _ => deterministic_match &= matches,
        }

        comparisons.push(MetricComparison {
            name: rec.name.clone(),
            kind: rec.kind,
            recorded: rec.value,
            replayed: now.value,
            delta,
            matches,
            note,
        });
    }

    ReplayReport {
        capsule_id: recorded.capsule_id.clone().unwrap_or_default(),
        // The claim is about deterministic metrics. An indicative drift is
        // reported but does not make the capsule a liar.
        reproduced: deterministic_match,
        deterministic_match,
        indicative_match,
        comparisons,
        environment_notes,
    }
}

pub fn run(format: Format, capsule_path: &std::path::Path, allow_drift: bool) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(capsule_path).map_err(|e| {
        Fixable::new(format!("cannot read {}: {e}", capsule_path.display()))
            .fix("pass the path to a capsule.json")
            .into_error()
    })?;
    let recorded = Capsule::from_json(&text).map_err(|e| {
        Fixable::new(format!("capsule is invalid: {e}"))
            .fix("regenerate it with `ccx measure`")
            .into_error()
    })?;

    // Find the solution the capsule came from: the capsule records source
    // hashes, and replay refuses if the source has changed, because a replay of
    // different code is not a replay.
    let dir = capsule_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| std::path::Path::new("."));
    let sol = Solution::load(dir)?;

    let mut source_notes = Vec::new();
    for (name, hash) in &recorded.source.files {
        let path = sol.root.join(name);
        let Ok(bytes) = std::fs::read(&path) else {
            return Err(Fixable::new(format!("source file {name} is missing"))
                .fix("replay requires the exact source the capsule was built from")
                .into_error());
        };
        let now = format!("sha256:{}", ccx_core::capsule::sha256_hex(&bytes));
        if &now != hash {
            return Err(Fixable::new(format!(
                "source file {name} has changed since the capsule was made"
            ))
            .fix("a replay of different code is not a replay: restore the source, or re-measure")
            .into_error());
        }
        source_notes.push(name.clone());
    }

    let pipeline = Pipeline::new(&sol)?;
    let policy = policy_for(&sol.problem);

    // Rebuild from the same source, then re-measure the same input.
    let program = pipeline.build_solution()?;
    let generator = pipeline.build_generator()?;
    let input = pipeline.generate_with(
        &generator,
        // Recover the size name from the recorded scale.
        &sol
            .problem
            .meta
            .sizes
            .iter()
            .find(|(_, s)| Some(**s) == recorded.input.size)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "naive".to_string()),
        recorded.input.seed,
    )?;

    let (outcome, counts) = pipeline.measure_once(&program, &input, &policy)?;
    let verdict = outcome.verdict(policy.memory_bytes);
    if verdict != Verdict::Accepted {
        return Err(Fixable::new(format!("the replay did not run cleanly: {verdict:?}"))
            .fix("the solution is not deterministic, or the environment changed")
            .into_error());
    }

    // Every metric the capsule records must be measured here too, or the
    // comparison reports a spurious difference for a metric that is in fact
    // stable. Allocations come from the run's own accounting, and the binary
    // size from the binary just rebuilt.
    let binary_size = std::fs::metadata(program.host_dir.join("sol"))
        .ok()
        .map(|m| m.len());
    let allocations = outcome.allocations;
    let measurement = Measurement {
        outcome,
        cachegrind: counts,
        allocations,
        binary_size,
        hardware: None,
    };

    let host = host_info();
    let report = compare(&recorded, &measurement, &host.fingerprint);

    cmd::emit(
        format,
        || {
            println!("replay of {}", &report.capsule_id[..16.min(report.capsule_id.len())]);
            for note in &report.environment_notes {
                println!("  note: {note}");
            }
            println!();
            println!("  {:<18} {:>14} {:>14}  {}", "metric", "recorded", "replayed", "match");
            for c in &report.comparisons {
                let mark = if c.matches { "ok" } else { "DIFF" };
                println!(
                    "  {:<18} {:>14} {:>14}  {mark}",
                    c.name,
                    format!("{:.0}", c.recorded),
                    format!("{:.0}", c.replayed),
                );
                if let Some(n) = &c.note {
                    println!("        {n}");
                }
            }
            println!();
            if report.reproduced {
                println!("  reproduced: deterministic metrics match exactly");
            } else {
                println!("  NOT reproduced: a deterministic metric changed");
            }
            if !report.indicative_match {
                println!("  indicative metrics drifted (expected across machines)");
            }
        },
        &report,
    )?;

    if !report.reproduced && !allow_drift {
        return Err(Fixable::new("the replay did not reproduce the capsule")
            .fix("a deterministic metric changing means the run was not reproducible")
            .fix("use --allow-drift to accept the difference and still exit successfully")
            .into_error());
    }
    Ok(())
}
