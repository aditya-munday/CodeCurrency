//! `ccx replay` — re-run a capsule and check whether it reproduces.
//!
//! This is the command the whole platform is built around. If a capsule cannot
//! be replayed to the same deterministic answer, the platform's claim is false,
//! and this command is where that would show up.
//!
//! Deterministic metrics match exactly when no uncertainty was measured.
//! Repeated samples are compared using their recorded confidence intervals.
//! Indicative metrics and getrusage counters remain machine-dependent and are compared the same way.

use crate::cmd::{self, Fixable};
use crate::context::{host_info, policy_for, Solution};
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::{Capsule, Metric, MetricKind, MetricMethod, ReferenceBuild, Verdict};
use ccx_core::engine::Measurement;
use ccx_core::problem::Judge;
use serde::Serialize;
use std::path::Path;

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
    pub machine_dependent_match: bool,
    pub comparisons: Vec<MetricComparison>,
    /// Environment differences that make exact reproduction impossible or
    /// expected, stated rather than hidden.
    pub environment_notes: Vec<String>,
}

fn verify_binary_digest(recorded: &str, current: &str) -> Result<(), String> {
    if recorded != ccx_core::capsule::LEGACY_BINARY_DIGEST_UNAVAILABLE && recorded != current {
        return Err(format!(
            "rebuilt executable digest differs from the capsule: recorded {recorded}, rebuilt {current}"
        ));
    }
    Ok(())
}

fn verify_reference_build(
    recorded: &ReferenceBuild,
    current_digest: &str,
    current_argv: &[String],
) -> Result<(), String> {
    verify_binary_digest(&recorded.binary_sha256, current_digest)?;
    if recorded.argv != current_argv {
        return Err("reference compiler arguments changed since measurement".into());
    }
    Ok(())
}

/// Compare a recorded capsule against a fresh measurement.
///
/// A deterministic metric must match exactly unless both capsules measured
/// uncertainty. Machine-dependent metrics are compared against their confidence range.
pub fn compare(recorded: &Capsule, replay: &[Metric], replay_host: &str) -> ReplayReport {
    let mut comparisons = Vec::new();
    let mut deterministic_match = true;
    let mut machine_dependent_match = true;
    let mut environment_notes = Vec::new();

    if recorded.host.fingerprint != replay_host {
        environment_notes.push(format!(
            "recorded on a different machine ({} vs {}); machine-dependent metrics are not directly comparable",
            &recorded.host.fingerprint[..12.min(recorded.host.fingerprint.len())],
            &replay_host[..12.min(replay_host.len())]
        ));
    }
    if recorded.harness.version != ccx_core::capsule::HARNESS_VERSION {
        environment_notes.push(format!(
            "harness version changed ({} vs {}); measurement and replay semantics may differ",
            recorded.harness.version,
            ccx_core::capsule::HARNESS_VERSION
        ));
    }

    for rec in &recorded.metrics {
        let Some(now) = replay.iter().find(|m| m.name == rec.name) else {
            comparisons.push(MetricComparison {
                name: rec.name.clone(),
                kind: rec.kind,
                recorded: rec.value,
                replayed: f64::NAN,
                delta: f64::NAN,
                matches: false,
                note: Some(if rec.machine_dependent() {
                    "machine-dependent metric is unavailable on this host".into()
                } else {
                    "metric is absent from the replay".into()
                }),
            });
            if rec.machine_dependent() {
                machine_dependent_match = false;
            } else {
                deterministic_match = false;
            }
            continue;
        };

        let delta = now.value - rec.value;
        let recorded_ci = rec.uncertainty.as_ref().map(|u| u.value).unwrap_or(0.0);
        let replay_ci = now.uncertainty.as_ref().map(|u| u.value).unwrap_or(0.0);
        let tolerance = 2.0 * (recorded_ci.powi(2) + replay_ci.powi(2)).sqrt();
        let matches = delta.abs() <= tolerance.max(0.5);
        let note = if matches && delta.abs() >= 0.5 && tolerance > 0.0 {
            Some(format!(
                "within replay tolerance of twice the combined 95% confidence half-width (+/- {:.3e})",
                tolerance
            ))
        } else if !matches && rec.machine_dependent() {
            Some(format!(
                "outside the recorded interval (+/- {:.3e}); machine-dependent metric",
                tolerance
            ))
        } else if !matches {
            Some("metric changed beyond the recorded repeatability interval".into())
        } else {
            None
        };

        if rec.machine_dependent() {
            machine_dependent_match &= matches;
        } else {
            deterministic_match &= matches;
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
        // The claim is about deterministic metrics. Machine-dependent drift is
        // reported but does not make the capsule a liar.
        reproduced: deterministic_match,
        deterministic_match,
        machine_dependent_match,
        comparisons,
        environment_notes,
    }
}

fn verify_toolchain_environment(
    current: &ccx_core::toolchain::Toolchain,
    recorded: &ccx_core::capsule::Toolchain,
) -> Result<bool, String> {
    if current.name != recorded.name
        || current.version != recorded.version
        || current.image != recorded.image
    {
        return Err("the selected compiler profile differs from the capsule".into());
    }
    match recorded.environment_fingerprint.as_deref() {
        Some(fingerprint) if fingerprint == current.environment_fingerprint => {
            Ok(current.image_digest != recorded.image_digest)
        }
        Some(_) => {
            Err("the pinned toolchain environment fingerprint differs from the capsule".into())
        }
        None if current.image_digest == recorded.image_digest => Ok(false),
        None => Err(
            "the exact local toolchain image digest required by this older capsule is unavailable"
                .into(),
        ),
    }
}

pub fn run(
    format: Format,
    capsule_path: &Path,
    solution_dir: Option<&Path>,
    allow_drift: bool,
) -> anyhow::Result<()> {
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
    let parent = capsule_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let dir = match solution_dir {
        Some(path) => path,
        None => parent
            .ancestors()
            .find(|path| path.join("ccx.toml").is_file())
            .ok_or_else(|| {
                Fixable::new("cannot find the solution directory for this capsule")
                    .fix("pass `--dir <solution-directory>` when the capsule is stored elsewhere")
                    .into_error()
            })?,
    };
    let mut sol = Solution::load(dir)?;
    sol.toolchain_name = recorded.toolchain.name;
    sol.opt_level = recorded.build.opt_level.clone();
    if sol.program_args != recorded.build.argv_program {
        return Err(Fixable::new("program arguments differ from the capsule")
            .fix("restore the exact `program_args` in ccx.toml or measure again")
            .into_error());
    }

    let mut source_notes = Vec::new();
    let recorded_problem_files: std::collections::BTreeMap<String, String> = recorded
        .source
        .files
        .iter()
        .filter_map(|(name, hash)| {
            name.strip_prefix("problem/")
                .map(|relative| (relative.to_string(), hash.clone()))
        })
        .collect();
    if !recorded_problem_files.is_empty() {
        let current_problem_files = hash_directory(&sol.problem.root)?;
        if current_problem_files != recorded_problem_files {
            return Err(Fixable::new("problem files differ from the capsule")
                .fix("restore the exact problem folder or create a new measurement")
                .into_error());
        }
    }
    for (name, hash) in &recorded.source.files {
        let path = match name.strip_prefix("problem/") {
            Some(relative) => sol.problem.root.join(relative),
            None => sol.root.join(name),
        };
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

    let statement = std::fs::read_to_string(sol.problem.statement_path())?;
    let statement_hash = ccx_core::capsule::sha256_hex(statement.as_bytes());
    if statement_hash != recorded.problem.statement_sha256 {
        return Err(
            Fixable::new("the problem statement changed since the capsule was made")
                .fix("restore the exact problem version or create a new measurement")
                .into_error(),
        );
    }

    let pipeline = Pipeline::new(&sol)?;
    let current_toolchain = sol.toolchain()?;
    let locally_rebuilt_image =
        verify_toolchain_environment(&current_toolchain, &recorded.toolchain).map_err(|error| {
            Fixable::new(error)
                .fix("select a toolchain profile with the same locked environment fingerprint")
                .fix("older capsules require their exact local image digest")
                .into_error()
        })?;
    if pipeline.build_argv() != recorded.build.argv {
        return Err(Fixable::new("the build command differs from the capsule")
            .fix("restore the exact solution configuration or measure again")
            .into_error());
    }
    if pipeline.expected_runtime_argv() != recorded.build.argv_runner {
        return Err(Fixable::new("the runtime command differs from the capsule")
            .fix("restore the exact run_command in ccx.toml or measure again")
            .into_error());
    }
    let policy = policy_for(&sol.problem);

    // Rebuild from the same source, then re-measure the same input.
    let (program, build_timings, initial_binary_hash) =
        pipeline.build_solution_repeated(recorded.build.build_repeat)?;
    let current_binary_digest = format!("sha256:{initial_binary_hash}");
    verify_binary_digest(&recorded.build.binary_sha256, &current_binary_digest)
    .map_err(|error| {
        Fixable::new(error)
            .fix("restore the exact compiler, flags, source files, and build inputs recorded by the capsule")
            .into_error()
    })?;
    let generator = pipeline.build_generator()?;
    let input = pipeline.generate_with(
        &generator,
        // Recover the size name from the recorded scale.
        &sol.problem
            .meta
            .sizes
            .iter()
            .find(|(_, s)| Some(**s) == recorded.input.size)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| "naive".to_string()),
        recorded.input.seed,
    )?;
    let input_hash = ccx_core::capsule::sha256_hex(&input);
    if recorded.build.stdin_sha256.as_deref() != Some(input_hash.as_str()) {
        return Err(
            Fixable::new("the regenerated input differs from the capsule")
                .fix("restore the original problem generator and metadata, or measure again")
                .into_error(),
        );
    }

    let (reference, reference_build_argv) = pipeline.build_reference_with_argv()?;
    if let Some(recorded_reference_build) = &recorded.reference_build {
        let reference_binary_hash =
            ccx_core::capsule::sha256_hex(&std::fs::read(reference.host_dir.join("ref"))?);
        let current_reference_digest = format!("sha256:{reference_binary_hash}");
        verify_reference_build(
            recorded_reference_build,
            &current_reference_digest,
            &reference_build_argv,
        )
        .map_err(|error| {
            Fixable::new(format!("reference build does not match the capsule: {error}"))
                .fix("restore the recorded problem reference, pinned image, and reference compiler recipe")
                .into_error()
        })?;
    }
    let expected = pipeline.reference_output_with(&reference, &input)?;
    let checker = if sol.problem.meta.judge == Judge::Checker {
        Some(pipeline.build_checker()?)
    } else {
        None
    };
    let correctness_probe = pipeline.run_program(&program, &input, &policy, false)?;
    let output_matches = if correctness_probe.verdict(policy.memory_bytes) != Verdict::Accepted {
        false
    } else if let Some(checker) = &checker {
        pipeline.check_output(
            checker,
            &input,
            &expected,
            correctness_probe.stdout.as_bytes(),
            &policy,
        )?
    } else {
        correctness_probe.stdout.as_bytes() == expected.as_slice()
    };
    if correctness_probe.verdict(policy.memory_bytes) != Verdict::Accepted || !output_matches {
        return Err(
            Fixable::new("the replayed solution no longer passes the reference")
                .fix("restore the original source and problem files, or create a new measurement")
                .into_error(),
        );
    }

    let replay_runs = recorded.repeatability.as_ref().map(|r| r.runs).unwrap_or(1);
    let paired_wall_clock = recorded
        .metrics
        .iter()
        .any(|metric| metric.name == "paired_speedup_ratio");
    let replay_build_timing = recorded.metrics.iter().any(|metric| {
        matches!(
            metric.name.as_str(),
            "build_wall_clock" | "build_user_cpu_time" | "build_system_cpu_time"
        )
    });
    let mut samples: std::collections::BTreeMap<String, Vec<f64>> =
        std::collections::BTreeMap::new();
    let mut templates: std::collections::BTreeMap<String, Metric> =
        std::collections::BTreeMap::new();
    if replay_build_timing {
        for (name, values) in replay_build_timing_samples(&build_timings) {
            if recorded.metrics.iter().any(|metric| metric.name == name) {
                let value = ccx_core::capsule::repeatability::median(&values)
                    .ok_or_else(|| anyhow::anyhow!("build metric {name} has no replay samples"))?;
                let metric = Metric {
                    name: name.clone(),
                    value,
                    unit: "nanoseconds".to_string(),
                    kind: MetricKind::Measured,
                    method: if name == "build_wall_clock" {
                        MetricMethod::WallClock
                    } else {
                        MetricMethod::Getrusage
                    },
                    uncertainty: ccx_core::capsule::repeatability::bootstrap_ci95_median(
                        &values, 1000, 0xC0FFEE,
                    ),
                };
                samples.insert(name.clone(), values);
                templates.insert(name, metric);
            }
        }
    }
    for run_index in 0..replay_runs {
        let binary_size = std::fs::metadata(program.host_dir.join("sol"))
            .ok()
            .map(|m| m.len());
        let (outcome, counts, reference_outcome) = if paired_wall_clock {
            let (outcome, counts, reference_outcome) = pipeline.measure_once_paired(
                &program,
                &reference,
                &input,
                &policy,
                run_index % 2 == 0,
            )?;
            (outcome, counts, Some(reference_outcome))
        } else {
            let (outcome, counts) = pipeline.measure_once(&program, &input, &policy)?;
            (outcome, counts, None)
        };
        let verdict = outcome.verdict(policy.memory_bytes);
        if verdict != Verdict::Accepted {
            return Err(
                Fixable::new(format!("the replay did not run cleanly: {verdict:?}"))
                    .fix("the solution is not deterministic, or the environment changed")
                    .into_error(),
            );
        }
        let output_matches = if let Some(checker) = &checker {
            pipeline.check_output(
                checker,
                &input,
                &expected,
                outcome.stdout.as_bytes(),
                &policy,
            )?
        } else {
            outcome.stdout.as_bytes() == expected.as_slice()
        };
        if !output_matches {
            return Err(
                Fixable::new("the replayed solution changed its output between samples")
                    .fix("restore deterministic solution behavior or create a new capsule")
                    .into_error(),
            );
        }
        if let Some(reference_outcome) = &reference_outcome {
            if reference_outcome.verdict(policy.memory_bytes) != Verdict::Accepted
                || reference_outcome.stdout.as_bytes() != expected.as_slice()
            {
                return Err(
                    Fixable::new("the replayed reference changed its output or failed")
                        .fix("restore the exact reference solution and problem files")
                        .into_error(),
                );
            }
        }
        // Every metric the capsule records must be measured here too, or the
        // comparison reports a spurious difference for a metric that is in fact
        // stable. Allocations come from the run's own accounting, and the binary
        // size from the binary just rebuilt.
        let measurement = Measurement {
            allocations: outcome.allocations,
            outcome: outcome.clone(),
            cachegrind: counts,
            binary_size,
            hardware: outcome.hardware,
        };
        let mut metrics = measurement.to_metrics();
        metrics.push(Metric {
            name: "wall_clock".to_string(),
            value: outcome.wall_ns as f64,
            unit: "nanoseconds".to_string(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: None,
        });
        if let Some(reference_outcome) = reference_outcome {
            metrics.push(Metric {
                name: "reference_wall_clock".to_string(),
                value: reference_outcome.wall_ns as f64,
                unit: "nanoseconds".to_string(),
                kind: MetricKind::Indicative,
                method: MetricMethod::WallClock,
                uncertainty: None,
            });
            metrics.push(Metric {
                name: "paired_speedup_ratio".to_string(),
                value: reference_outcome.wall_ns as f64 / outcome.wall_ns.max(1) as f64,
                unit: "ratio".to_string(),
                kind: MetricKind::Indicative,
                method: MetricMethod::WallClock,
                uncertainty: None,
            });
        }
        for metric in metrics {
            samples
                .entry(metric.name.clone())
                .or_default()
                .push(metric.value);
            templates.entry(metric.name.clone()).or_insert(metric);
        }
    }
    let replay_metrics: Vec<Metric> = templates
        .into_iter()
        .filter_map(|(name, mut metric)| {
            let values = samples.get(&name)?;
            metric.value = ccx_core::capsule::repeatability::median(values)?;
            metric.uncertainty =
                ccx_core::capsule::repeatability::bootstrap_ci95_median(values, 1000, 0xC0FFEE);
            Some(metric)
        })
        .collect();

    let host = host_info();
    let mut report = compare(&recorded, &replay_metrics, &host.fingerprint);
    if recorded.build.binary_sha256 == ccx_core::capsule::LEGACY_BINARY_DIGEST_UNAVAILABLE {
        report.environment_notes.push(
            "this migrated capsule predates executable digests; replay cannot confirm identical binary bytes".into(),
        );
    }
    if locally_rebuilt_image {
        report.environment_notes.push(
            "the local Docker image ID differs; its pinned toolchain environment fingerprint matches"
                .into(),
        );
    }
    if recorded.reference_build.is_none()
        && recorded
            .metrics
            .iter()
            .any(|metric| metric.name == "paired_speedup_ratio")
    {
        report.environment_notes.push(
            "this harness-0.12.0 capsule predates reference executable digests; paired timing is replayed without a reference binary identity check".into(),
        );
    }

    cmd::emit(
        format,
        || {
            println!(
                "replay of {}",
                &report.capsule_id[..16.min(report.capsule_id.len())]
            );
            for note in &report.environment_notes {
                println!("  note: {note}");
            }
            println!();
            println!(
                "  {:<18} {:>14} {:>14}  {}",
                "metric", "recorded", "replayed", "match"
            );
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
                println!("  reproduced: deterministic metrics match");
            } else {
                println!("  NOT reproduced: a deterministic metric changed");
            }
            if report.machine_dependent_match {
                println!("  machine-dependent metrics match their recorded intervals");
            } else {
                println!("  machine-dependent metrics drifted beyond their recorded intervals");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_dependent_os_drift_does_not_break_deterministic_replay() {
        let source = include_str!("../../../ccx-core/tests/fixtures/capsule-v2.json");
        let mut recorded = Capsule::from_json(source).unwrap();
        recorded.metrics.push(Metric::count(
            "user_cpu_time",
            10,
            ccx_core::capsule::MetricKind::Measured,
            MetricMethod::Getrusage,
        ));
        let mut replay = recorded.metrics.clone();
        replay
            .iter_mut()
            .find(|metric| metric.name == "user_cpu_time")
            .unwrap()
            .value = 20.0;
        let report = compare(&recorded, &replay, "other-host");
        assert!(report.deterministic_match);
        assert!(!report.machine_dependent_match);
        assert!(report.reproduced);
    }

    #[test]
    fn equivalent_rebuilt_image_is_accepted_by_environment_fingerprint() {
        let current = ccx_core::toolchain::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:current".into(),
            environment_fingerprint: "sha256:environment".into(),
            libc: "glibc 2.41".into(),
            compiler: "gcc".into(),
            compiler_binaries: vec!["/usr/bin/gcc".into()],
            compiler_sha256: "a".repeat(64),
        };
        let recorded = ccx_core::capsule::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:recorded".into(),
            environment_fingerprint: Some("sha256:environment".into()),
            libc: "glibc 2.41".into(),
        };
        assert_eq!(verify_toolchain_environment(&current, &recorded), Ok(true));
    }

    #[test]
    fn different_environment_fingerprint_is_rejected() {
        let current = ccx_core::toolchain::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:current".into(),
            environment_fingerprint: "sha256:current-env".into(),
            libc: "glibc 2.41".into(),
            compiler: "gcc".into(),
            compiler_binaries: vec!["/usr/bin/gcc".into()],
            compiler_sha256: "a".repeat(64),
        };
        let recorded = ccx_core::capsule::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:recorded".into(),
            environment_fingerprint: Some("sha256:recorded-env".into()),
            libc: "glibc 2.41".into(),
        };
        assert!(verify_toolchain_environment(&current, &recorded).is_err());
    }

    #[test]
    fn executable_digest_must_match_when_capsule_records_it() {
        let digest = format!("sha256:{}", "a".repeat(64));
        assert!(verify_binary_digest(&digest, &digest).is_ok());
        assert!(verify_binary_digest(&digest, "sha256:other").is_err());
        assert!(verify_binary_digest(
            ccx_core::capsule::LEGACY_BINARY_DIGEST_UNAVAILABLE,
            "sha256:current"
        )
        .is_ok());
    }

    #[test]
    fn reference_digest_and_compiler_arguments_must_match() {
        let recorded = ReferenceBuild {
            binary_sha256: format!("sha256:{}", "a".repeat(64)),
            argv: vec!["g++".into(), "-O2".into(), "/src0/reference.cpp".into()],
        };
        assert!(verify_reference_build(&recorded, &recorded.binary_sha256, &recorded.argv).is_ok());
        assert!(verify_reference_build(&recorded, "sha256:other", &recorded.argv).is_err());
        assert!(verify_reference_build(
            &recorded,
            &recorded.binary_sha256,
            &["g++".into(), "-O3".into()]
        )
        .is_err());
    }

    #[test]
    fn legacy_capsule_requires_the_exact_image_digest() {
        let current = ccx_core::toolchain::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:current".into(),
            environment_fingerprint: "sha256:environment".into(),
            libc: "glibc 2.41".into(),
            compiler: "gcc".into(),
            compiler_binaries: vec!["/usr/bin/gcc".into()],
            compiler_sha256: "a".repeat(64),
        };
        let recorded = ccx_core::capsule::Toolchain {
            name: ccx_core::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:recorded".into(),
            environment_fingerprint: None,
            libc: "glibc 2.41".into(),
        };
        assert!(verify_toolchain_environment(&current, &recorded).is_err());
    }

    #[test]
    fn unavailable_hardware_counters_do_not_invalidate_deterministic_replay() {
        let source = include_str!("../../../ccx-core/tests/fixtures/capsule-v2.json");
        let mut recorded = Capsule::from_json(source).unwrap();
        recorded.metrics.push(Metric::count(
            "instructions_hw",
            900,
            ccx_core::capsule::MetricKind::Measured,
            MetricMethod::Perf,
        ));
        let replay = recorded
            .metrics
            .iter()
            .filter(|metric| metric.name != "instructions_hw")
            .cloned()
            .collect::<Vec<_>>();
        let report = compare(&recorded, &replay, &recorded.host.fingerprint);
        assert!(report.reproduced);
        assert!(report.deterministic_match);
        assert!(!report.machine_dependent_match);
        let hardware = report
            .comparisons
            .iter()
            .find(|metric| metric.name == "instructions_hw")
            .unwrap();
        assert!(!hardware.matches);
        assert_eq!(
            hardware.note.as_deref(),
            Some("machine-dependent metric is unavailable on this host")
        );
    }
}

fn hash_directory(
    root: &std::path::Path,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.')
                    || matches!(name.as_ref(), "target" | "build" | "dist" | "node_modules")
                {
                    continue;
                }
                visit(root, &path, files)?;
            } else if kind.is_file() {
                let relative = path
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                let bytes = std::fs::read(path)?;
                files.insert(
                    relative,
                    format!("sha256:{}", ccx_core::capsule::sha256_hex(&bytes)),
                );
            }
        }
        Ok(())
    }
    let mut files = std::collections::BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}

fn replay_build_timing_samples(
    timings: &[crate::pipeline::BuildTiming],
) -> std::collections::BTreeMap<String, Vec<f64>> {
    let mut samples = std::collections::BTreeMap::from([(
        "build_wall_clock".to_string(),
        timings.iter().map(|timing| timing.wall_ns as f64).collect(),
    )]);
    for (name, values) in [
        (
            "build_user_cpu_time",
            timings
                .iter()
                .map(|timing| timing.user_cpu_ns)
                .collect::<Option<Vec<_>>>(),
        ),
        (
            "build_system_cpu_time",
            timings
                .iter()
                .map(|timing| timing.system_cpu_ns)
                .collect::<Option<Vec<_>>>(),
        ),
    ] {
        if let Some(values) = values {
            samples.insert(
                name.to_string(),
                values.into_iter().map(|value| value as f64).collect(),
            );
        }
    }
    samples
}
