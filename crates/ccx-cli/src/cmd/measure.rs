//! `ccx measure` — produce a capsule: the complete, replayable record of one
//! measured run.
//!
//! A capsule is the product. Anyone holding it can replay the run and get the
//! same deterministic answer, which is the whole claim of the platform.
//!
//! Every metric carries its own label — measured, simulated, or indicative —
//! and the capsule is refused if that labelling is inconsistent. An indicative
//! metric without an uncertainty interval is a bug, not a rough number, because
//! it would present machine noise as a result.

use crate::cmd::{self, Fixable};
use crate::context::{host_info, policy_for, Solution};
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::{
    fixed_env_map, hash_source_files, Build, Capsule, Harness, InputSpec, Metric, MetricKind,
    MetricMethod, ProblemRef, ReferenceBuild, Repeatability, SeedSet, SourceRef, Spread, Toolchain,
    UncertaintyKind, Verdict, CURRENT_SCHEMA_VERSION, DEFAULT_BUILD_REPEAT, HARNESS_VERSION,
};
use ccx_core::engine::Measurement;
use ccx_core::explain::{self, Hint, Ruleset};
use ccx_core::problem::Judge;
use ccx_core::toolchain::pinned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// What `measure` prints besides the capsule.
#[derive(Debug, Clone, Serialize)]
pub struct MeasureReport {
    pub capsule_path: String,
    pub capsule_id: String,
    pub input_sha256: String,
    pub execution_target: String,
    pub host: ccx_core::capsule::Host,
    pub toolchain: Toolchain,
    pub verdict: Verdict,
    pub ranked_metric: String,
    pub runs: u32,
    pub repeatability: Option<Repeatability>,
    pub build_repeatability: Option<Repeatability>,
    pub metrics: Vec<MetricSummary>,
    pub hints: Vec<Hint>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricSummary {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub kind: MetricKind,
    pub method: MetricMethod,
    pub repeatability: Option<Spread>,
    pub uncertainty_kind: Option<UncertaintyKind>,
    /// Present for indicative metrics and for any metric measured more than once.
    /// Half-width of the 95% interval of the median.
    pub uncertainty: Option<f64>,
}

pub fn run(
    format: Format,
    dir: &Path,
    repeat: u32,
    out: Option<&Path>,
    size: &str,
    seed: u64,
) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let report = measure_solution(&sol, repeat, out, size, seed)?;
    emit_report(format, &sol.problem.meta.id, &report)
}

pub fn measure_solution(
    sol: &Solution,
    repeat: u32,
    out: Option<&Path>,
    size: &str,
    seed: u64,
) -> anyhow::Result<MeasureReport> {
    if repeat == 0 || repeat > 100 {
        return Err(Fixable::new(format!(
            "repeat count {repeat} is outside the supported range"
        ))
        .fix("choose a repeat count from 1 to 100")
        .into_error());
    }
    if sol.language == ccx_core::capsule::Language::Go && repeat < 20 {
        return Err(Fixable::new("Go measurements require at least 20 runs")
            .fix("Go runtime counters vary between fresh processes; use `--repeat 20` or more")
            .into_error());
    }
    let pipeline = Pipeline::new(&sol)?;
    let policy = policy_for(&sol.problem);
    let rules = Ruleset::default();

    let scale = *sol.problem.meta.sizes.get(size).ok_or_else(|| {
        Fixable::new(format!("problem has no size named {size:?}"))
            .fix(format!(
                "available sizes: {}",
                sol.problem
                    .meta
                    .sizes
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .into_error()
    })?;

    // Build once; every repetition runs the same bytes.
    let (program, build_timings, initial_binary_hash) =
        pipeline.build_solution_repeated(DEFAULT_BUILD_REPEAT)?;
    let build_samples = build_timing_samples(&build_timings);
    let generator = pipeline.build_generator()?;
    let (reference, reference_build_argv) = pipeline.build_reference_with_argv()?;
    let reference_binary_hash =
        ccx_core::capsule::sha256_hex(&std::fs::read(reference.host_dir.join("ref"))?);
    let input = pipeline.generate_with(&generator, size, seed)?;

    // Correctness gates measurement: a wrong answer has no meaningful cost.
    let expected = pipeline.reference_output_with(&reference, &input)?;
    let probe = pipeline.run_program(&program, &input, &policy, false)?;
    let checker = if sol.problem.meta.judge == Judge::Checker {
        Some(pipeline.build_checker()?)
    } else {
        None
    };
    let probe_verdict = probe.verdict(policy.memory_bytes);
    if probe_verdict != Verdict::Accepted {
        return Err(Fixable::new(format!(
            "the solution did not run cleanly: {probe_verdict:?} (exit {}, signal {})",
            probe.exit_status, probe.signal
        ))
        .fix("run `ccx test` to see which cases fail")
        .into_error());
    }
    let output_matches = match &checker {
        Some(checker) => {
            pipeline.check_output(checker, &input, &expected, probe.stdout.as_bytes(), &policy)?
        }
        None => probe.stdout.as_bytes() == expected.as_slice(),
    };
    if !output_matches {
        return Err(
            Fixable::new("the solution produced a wrong answer on this input")
                .fix("measurement is refused for an incorrect solution: fix correctness first")
                .fix("run `ccx test` to see the first differing line")
                .into_error(),
        );
    }

    // Repeat the measured run so the spread is real rather than assumed.
    let runs = repeat.max(2);
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    // Run Cachegrind for every repetition. Otherwise the capsule would claim
    // repeatability for native metrics while deterministic instruction/cache/
    // branch metrics had only one sample and no measured spread.
    let mut last_measurement: Option<Measurement> = None;

    for i in 0..runs {
        let (outcome, counts, reference_outcome) =
            pipeline.measure_once_paired(&program, &reference, &input, &policy, i % 2 == 0)?;

        let verdict = outcome.verdict(policy.memory_bytes);
        if verdict != Verdict::Accepted {
            return Err(Fixable::new(format!(
                "run {i} of {runs} did not complete cleanly: {verdict:?}"
            ))
            .fix("a flaky solution cannot be measured: make it deterministic first")
            .into_error());
        }
        if reference_outcome.verdict(policy.memory_bytes) != Verdict::Accepted
            || reference_outcome.stdout.as_bytes() != expected.as_slice()
        {
            return Err(Fixable::new(format!(
                "reference run {i} of {runs} changed its output or failed"
            ))
            .fix("paired timing requires the reference to remain correct on every sample")
            .into_error());
        }
        let pair_matches = match &checker {
            Some(checker) => pipeline.check_output(
                checker,
                &input,
                &expected,
                outcome.stdout.as_bytes(),
                &policy,
            )?,
            None => outcome.stdout.as_bytes() == expected.as_slice(),
        };
        if !pair_matches {
            return Err(Fixable::new(format!(
                "solution run {i} of {runs} produced a wrong answer"
            ))
            .fix("a flaky solution cannot be measured: make it deterministic first")
            .into_error());
        }

        let binary_size = std::fs::metadata(program.host_dir.join("sol"))
            .ok()
            .map(|m| m.len());

        let measurement = Measurement {
            outcome: outcome.clone(),
            cachegrind: counts,
            allocations: outcome.allocations,
            binary_size,
            hardware: outcome.hardware,
        };
        let mut run_metrics = measurement.to_metrics();
        run_metrics.push(Metric {
            name: "wall_clock".to_string(),
            value: outcome.wall_ns as f64,
            unit: "nanoseconds".to_string(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: None,
        });
        for m in run_metrics {
            samples.entry(m.name.clone()).or_default().push(m.value);
        }
        samples
            .entry("reference_wall_clock".to_string())
            .or_default()
            .push(reference_outcome.wall_ns as f64);
        samples
            .entry("paired_speedup_ratio".to_string())
            .or_default()
            .push(reference_outcome.wall_ns as f64 / outcome.wall_ns.max(1) as f64);
        last_measurement = Some(measurement);
    }

    let measurement = last_measurement.ok_or_else(|| {
        Fixable::new("no measurement was taken")
            .fix("this is an engine bug")
            .into_error()
    })?;

    // A single run has no spread; recording the run count is more honest than
    // reporting a zero-width interval.
    let mut metrics = measurement.to_metrics();
    metrics.push(Metric {
        name: "wall_clock".to_string(),
        value: measurement.outcome.wall_ns as f64,
        unit: "nanoseconds".to_string(),
        kind: MetricKind::Indicative,
        method: MetricMethod::WallClock,
        uncertainty: None,
    });
    metrics.push(Metric {
        name: "reference_wall_clock".to_string(),
        value: 0.0,
        unit: "nanoseconds".to_string(),
        kind: MetricKind::Indicative,
        method: MetricMethod::WallClock,
        uncertainty: None,
    });
    metrics.push(Metric {
        name: "paired_speedup_ratio".to_string(),
        value: 0.0,
        unit: "ratio".to_string(),
        kind: MetricKind::Indicative,
        method: MetricMethod::WallClock,
        uncertainty: None,
    });
    let build_repeatability = (build_timings.len() > 1).then(|| Repeatability {
        runs: build_timings.len() as u32,
        per_metric: ccx_core::capsule::repeatability::summarise(&build_samples),
    });
    for (name, values) in &build_samples {
        let Some(value) = ccx_core::capsule::repeatability::median(values) else {
            continue;
        };
        metrics.push(Metric {
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
                values, 1000, 0xC0FFEE,
            ),
        });
    }
    let repeatability = (runs > 1).then(|| {
        let per_metric = ccx_core::capsule::repeatability::summarise(&samples);
        ccx_core::capsule::Repeatability { runs, per_metric }
    });

    // Attach an uncertainty interval to any metric measured more than once.
    // The interval is a seeded bootstrap of the median, so it is reproducible:
    // replaying the same samples yields the same interval.
    if repeatability.is_some() {
        for m in &mut metrics {
            let Some(values) = samples.get(&m.name) else {
                continue;
            };
            if values.len() < 2 {
                continue;
            }
            if let Some(median) = ccx_core::capsule::repeatability::median(values) {
                m.value = median;
            }
            if let Some(ci) =
                ccx_core::capsule::repeatability::bootstrap_ci95_median(values, 1000, 0xC0FFEE)
            {
                m.uncertainty = Some(ci);
            }
        }
    }

    // Peak memory and wall time are machine-dependent, so they are indicative
    // by definition and must carry a spread.
    for m in &mut metrics {
        if m.name == "peak_memory" {
            m.kind = MetricKind::Indicative;
        }
    }

    let tc = pinned(sol.toolchain_name).ok_or_else(|| {
        Fixable::new("no pinned toolchain found")
            .fix("run scripts/pin-toolchains.sh")
            .into_error()
    })?;

    let host = host_info();
    let mut source_files: BTreeMap<String, String> = sol
        .project_files
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                format!("sha256:{}", ccx_core::capsule::sha256_hex(v.as_bytes())),
            )
        })
        .collect();
    collect_problem_files(&sol.problem.root, &sol.problem.root, &mut source_files)?;

    let statement = std::fs::read_to_string(sol.problem.statement_path()).unwrap_or_default();

    let mut capsule = Capsule {
        schema_version: CURRENT_SCHEMA_VERSION,
        capsule_id: None,
        created_at: ccx_core::capsule::utc_timestamp_now(),
        problem: ProblemRef {
            id: sol.problem.meta.id.clone(),
            statement_sha256: ccx_core::capsule::sha256_hex(statement.as_bytes()),
        },
        source: SourceRef {
            language: sol.language,
            entry: if sol.language == ccx_core::capsule::Language::Rust
                && sol.root.join("main.rs").is_file()
            {
                "main.rs".to_string()
            } else if sol.language == ccx_core::capsule::Language::Go
                && sol.root.join("main.go").is_file()
            {
                "main.go".to_string()
            } else {
                sol.sources.keys().next().cloned().unwrap_or_else(|| {
                    format!("main.{}", crate::pipeline::extension_for(sol.language))
                })
            },
            files: source_files.clone(),
            source_sha256: hash_source_files(&source_files),
        },
        toolchain: Toolchain {
            name: tc.name,
            version: tc.version.clone(),
            image: tc.image.clone(),
            image_digest: tc.image_digest.clone(),
            environment_fingerprint: Some(tc.environment_fingerprint.clone()),
            libc: tc.libc.clone(),
        },
        build: Build {
            argv: vec![],
            opt_level: if sol.language == ccx_core::capsule::Language::AsmX86_64 {
                "none".to_string()
            } else {
                sol.opt_level.clone()
            },
            binary_sha256: format!("sha256:{initial_binary_hash}"),
            build_repeat: build_timings.len() as u32,
            env: {
                let mut env = fixed_env_map();
                if sol.language == ccx_core::capsule::Language::Go {
                    env.insert("GO111MODULE".to_string(), "off".to_string());
                    env.insert("GOTOOLCHAIN".to_string(), "local".to_string());
                    env.insert("GOPROXY".to_string(), "off".to_string());
                    env.insert("GOSUMDB".to_string(), "off".to_string());
                    env.insert("GOENV".to_string(), "off".to_string());
                    env.insert("GOROOT".to_string(), "/usr/local/go".to_string());
                    env.insert("GOCACHE".to_string(), "/work/scratch/go-cache".to_string());
                    env.insert("GOTMPDIR".to_string(), "/work/scratch".to_string());
                    env.insert("GOMAXPROCS".to_string(), "1".to_string());
                    env.insert("GODEBUG".to_string(), "randautoseed=0".to_string());
                }
                env
            },
            argv_program: program.args.clone(),
            argv_runner: program.runtime_argv.clone().unwrap_or_default(),
            stdin_sha256: Some(ccx_core::capsule::sha256_hex(&input)),
            stack_bytes: 8 * 1024 * 1024,
        },
        reference_build: Some(ReferenceBuild {
            binary_sha256: format!("sha256:{reference_binary_hash}"),
            argv: reference_build_argv,
        }),
        harness: Harness {
            version: HARNESS_VERSION.to_string(),
            metric_method: MetricMethod::Cachegrind,
            ruleset: rules.id(),
        },
        input: InputSpec {
            generator: "generator.c".to_string(),
            seed,
            seed_set: SeedSet::Public,
            size: Some(scale),
        },
        host,
        verdict: Verdict::Accepted,
        metrics,
        repeatability,
        build_repeatability,
    };
    // Record how the binary was produced: without it a replay cannot rebuild
    // the same bytes, and the capsule would not be self-contained.
    capsule.build.argv = pipeline.build_argv();
    capsule.seal()?;
    capsule.validate()?;

    let capsule_path = out
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| sol.root.join("capsule.json"));
    if let Some(parent) = capsule_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&capsule_path, capsule.to_json_pretty()?)?;

    let hints = explain::explain(&capsule, &rules);

    let report = MeasureReport {
        capsule_path: capsule_path.display().to_string(),
        capsule_id: capsule.capsule_id.clone().unwrap_or_default(),
        input_sha256: capsule.build.stdin_sha256.clone().unwrap_or_default(),
        execution_target: "linux/x86_64".to_string(),
        host: capsule.host.clone(),
        toolchain: capsule.toolchain.clone(),
        verdict: capsule.verdict,
        ranked_metric: format!("{:?}", sol.problem.meta.ranked_metric).to_lowercase(),
        runs,
        repeatability: capsule.repeatability.clone(),
        build_repeatability: capsule.build_repeatability.clone(),
        metrics: capsule
            .metrics
            .iter()
            .map(|m| MetricSummary {
                name: m.name.clone(),
                value: m.value,
                unit: m.unit.clone(),
                kind: m.kind,
                method: m.method,
                repeatability: capsule
                    .repeatability
                    .as_ref()
                    .and_then(|summary| summary.per_metric.get(&m.name).cloned())
                    .or_else(|| {
                        capsule
                            .build_repeatability
                            .as_ref()
                            .and_then(|summary| summary.per_metric.get(&m.name).cloned())
                    }),
                uncertainty_kind: m.uncertainty.as_ref().map(|u| u.kind),
                uncertainty: m.uncertainty.as_ref().map(|u| u.value),
            })
            .collect(),
        hints: hints.clone(),
    };

    Ok(report)
}

pub fn emit_report(format: Format, problem: &str, report: &MeasureReport) -> anyhow::Result<()> {
    cmd::emit(
        format,
        || {
            println!(
                "capsule {}",
                &report.capsule_id[..16.min(report.capsule_id.len())]
            );
            println!("  problem   {problem}");
            println!("  ranked on {}", report.ranked_metric);
            println!("  runs      {}", report.runs);
            println!(
                "  target    {} | {} {} | {} | {} CPUs | frequency policy {}",
                report.execution_target,
                report.host.cpu_model,
                report.host.os,
                report.host.kernel,
                report.host.nproc,
                report.host.frequency_policy
            );
            println!(
                "  compiler  {} {} | environment {} | host {}",
                report.toolchain.name.profile_id(),
                report.toolchain.version,
                report
                    .toolchain
                    .environment_fingerprint
                    .as_deref()
                    .unwrap_or("legacy image digest only"),
                report.host.fingerprint
            );
            println!();
            println!(
                "  {:<18} {:>14}  {:<10} {:<12} {}",
                "metric", "value", "unit", "kind", "method"
            );
            for m in &report.metrics {
                let value = format_metric_value(m.value, &m.unit);
                let spread = m
                    .uncertainty
                    .map(|hw| format!("  95% CI half-width +/- {hw:.3e}"))
                    .unwrap_or_default();
                println!(
                    "  {:<26} {:>14}  {:<18} {:<12} {}{spread}",
                    m.name,
                    value,
                    m.unit,
                    format!("{:?}", m.kind).to_lowercase(),
                    format!("{:?}", m.method).to_lowercase(),
                );
                if let Some(repeatability) = &m.repeatability {
                    let tails = match (repeatability.p95, repeatability.p99) {
                        (Some(p95), Some(p99)) => format!(
                            " p95={} p99={}",
                            format_metric_value(p95, &m.unit),
                            format_metric_value(p99, &m.unit)
                        ),
                        _ => String::new(),
                    };
                    println!(
                        "    n={}{} range=[{}, {}] relative spread={:.4}%",
                        repeatability.samples,
                        tails,
                        format_metric_value(repeatability.min, &m.unit),
                        format_metric_value(repeatability.max, &m.unit),
                        repeatability.spread_pct
                    );
                }
            }
            if !report.hints.is_empty() {
                println!();
                println!("  what the counters say:");
                for h in &report.hints {
                    println!("    • {}", h.message);
                }
            }
            println!();
            println!("  wrote {}", report.capsule_path);
        },
        &report,
    )
}

fn format_metric_value(value: f64, unit: &str) -> String {
    if matches!(unit, "ratio" | "instructions/cycle") {
        format!("{value:.6}")
    } else if unit == "hertz" || value.abs() >= 1e6 {
        format!("{value:.3e}")
    } else {
        format!("{value:.0}")
    }
}

fn build_timing_samples(timings: &[crate::pipeline::BuildTiming]) -> BTreeMap<String, Vec<f64>> {
    let mut samples = BTreeMap::from([(
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

#[cfg(test)]
mod tests {
    use super::format_metric_value;

    #[test]
    fn ratios_and_ipc_keep_fractional_precision() {
        assert_eq!(format_metric_value(0.037125, "ratio"), "0.037125");
        assert_eq!(format_metric_value(1.875, "instructions/cycle"), "1.875000");
    }

    #[test]
    fn counts_remain_compact_and_frequency_is_not_rounded_to_zero() {
        assert_eq!(format_metric_value(12345.0, "count"), "12345");
        assert_eq!(format_metric_value(2_400_000_000.0, "hertz"), "2.400e9");
    }
}

fn collect_problem_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    files: &mut BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
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
            collect_problem_files(root, &path, files)?;
        } else if kind.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(path)?;
            files.insert(
                format!("problem/{relative}"),
                format!("sha256:{}", ccx_core::capsule::sha256_hex(&bytes)),
            );
        }
    }
    Ok(())
}
