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
    fixed_env_map, hash_source_files, Build, Capsule, Harness, InputSpec, MetricKind,
    MetricMethod, ProblemRef, SeedSet, SourceRef, Toolchain, Verdict, CURRENT_SCHEMA_VERSION,
    HARNESS_VERSION,
};
use ccx_core::engine::Measurement;
use ccx_core::explain::{self, Hint, Ruleset};
use ccx_core::toolchain::pinned;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// What `measure` prints besides the capsule.
#[derive(Debug, Clone, Serialize)]
pub struct MeasureReport {
    pub capsule_path: String,
    pub capsule_id: String,
    pub verdict: Verdict,
    pub ranked_metric: String,
    pub runs: u32,
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
    let pipeline = Pipeline::new(&sol)?;
    let policy = policy_for(&sol.problem);
    let rules = Ruleset::default();

    let scale = *sol.problem.meta.sizes.get(size).ok_or_else(|| {
        Fixable::new(format!("problem has no size named {size:?}"))
            .fix(format!(
                "available sizes: {}",
                sol.problem.meta.sizes.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
            .into_error()
    })?;

    // Build once; every repetition runs the same bytes.
    let program = pipeline.build_solution()?;
    let generator = pipeline.build_generator()?;
    let reference = pipeline.build_reference()?;
    let input = pipeline.generate_with(&generator, size, seed)?;

    // Correctness gates measurement: a wrong answer has no meaningful cost.
    let expected = pipeline.reference_output_with(&reference, &input)?;
    let probe = pipeline.run_program(&program, &input, &policy, false)?;
    let probe_verdict = probe.verdict(policy.memory_bytes);
    if probe_verdict != Verdict::Accepted {
        return Err(Fixable::new(format!(
            "the solution did not run cleanly: {probe_verdict:?} (exit {}, signal {})",
            probe.exit_status, probe.signal
        ))
        .fix("run `ccx test` to see which cases fail")
        .into_error());
    }
    if probe.stdout.as_bytes() != expected.as_slice() {
        return Err(Fixable::new("the solution produced a wrong answer on this input")
            .fix("measurement is refused for an incorrect solution: fix correctness first")
            .fix("run `ccx test` to see the first differing line")
            .into_error());
    }

    // Repeat the measured run so the spread is real rather than assumed.
    let runs = repeat.max(1);
    let mut samples: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    // The cachegrind pass runs once, on the first iteration, so its result has
    // to be carried forward: the capsule must not be built from the *last*
    // measurement, which by construction has no simulated counters.
    let mut counters: Option<ccx_core::measure::CachegrindCounts> = None;
    let mut last_measurement: Option<Measurement> = None;
    let mut peak_rss = 0u64;
    let mut allocations = None;

    for i in 0..runs {
        // Only the first run needs the (slow) cachegrind pass; the rest exist
        // to measure the spread of the cheap, honest metrics.
        let measure_counters = i == 0;
        let (outcome, counts) = if measure_counters {
            pipeline.measure_once(&program, &input, &policy)?
        } else {
            (pipeline.run_program(&program, &input, &policy, false)?, None)
        };

        let verdict = outcome.verdict(policy.memory_bytes);
        if verdict != Verdict::Accepted {
            return Err(Fixable::new(format!(
                "run {i} of {runs} did not complete cleanly: {verdict:?}"
            ))
            .fix("a flaky solution cannot be measured: make it deterministic first")
            .into_error());
        }

        if counters.is_none() {
            counters = counts;
        }
        peak_rss = peak_rss.max(outcome.peak_rss_bytes);
        if allocations.is_none() {
            allocations = outcome.allocations;
        }
        let binary_size = std::fs::metadata(program.host_dir.join("sol"))
            .ok()
            .map(|m| m.len());

        let measurement = Measurement {
            outcome: outcome.clone(),
            cachegrind: counts,
            allocations: outcome.allocations,
            binary_size,
            hardware: None,
        };
        for m in measurement.to_metrics() {
            samples.entry(m.name.clone()).or_default().push(m.value);
        }
        last_measurement = Some(measurement);
    }

    let mut measurement = last_measurement.ok_or_else(|| {
        Fixable::new("no measurement was taken").fix("this is an engine bug").into_error()
    })?;
    // Re-attach the counters from the measured iteration.
    measurement.cachegrind = counters;

    // A single run has no spread; recording the run count is more honest than
    // reporting a zero-width interval.
    let mut metrics = measurement.to_metrics();
    let repeatability = (runs > 1).then(|| {
        let per_metric = ccx_core::capsule::repeatability::summarise(&samples);
        ccx_core::capsule::Repeatability { runs, per_metric }
    });

    // Attach an uncertainty interval to any metric measured more than once.
    // The interval is a seeded bootstrap of the median, so it is reproducible:
    // replaying the same samples yields the same interval.
    if repeatability.is_some() {
        for m in &mut metrics {
            let Some(values) = samples.get(&m.name) else { continue };
            if values.len() < 2 {
                continue;
            }
            if let Some(ci) = ccx_core::capsule::repeatability::bootstrap_ci95_median(values, 1000, 0xC0FFEE) {
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
        Fixable::new("no pinned toolchain found").fix("run scripts/pin-toolchains.sh").into_error()
    })?;

    let host = host_info();
    let source_files: BTreeMap<String, String> = sol
        .sources
        .iter()
        .map(|(k, v)| (k.clone(), format!("sha256:{}", ccx_core::capsule::sha256_hex(v.as_bytes()))))
        .collect();

    let statement = std::fs::read_to_string(sol.problem.statement_path()).unwrap_or_default();

    let mut capsule = Capsule {
        schema_version: CURRENT_SCHEMA_VERSION,
        capsule_id: None,
        created_at: String::new(),
        problem: ProblemRef {
            id: sol.problem.meta.id.clone(),
            statement_sha256: ccx_core::capsule::sha256_hex(statement.as_bytes()),
        },
        source: SourceRef {
            language: sol.language,
            entry: sol
                .sources
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| format!("main.{}", crate::pipeline::extension_for(sol.language))),
            files: source_files.clone(),
            source_sha256: hash_source_files(&source_files),
        },
        toolchain: Toolchain {
            name: tc.name,
            version: tc.version.clone(),
            image: tc.image.clone(),
            image_digest: tc.image_digest.clone(),
            libc: tc.libc.clone(),
        },
        build: Build {
            argv: vec![],
            opt_level: sol.opt_level.clone(),
            env: fixed_env_map(),
            argv_program: vec![],
            stdin_sha256: Some(ccx_core::capsule::sha256_hex(&input)),
            stack_bytes: 8 * 1024 * 1024,
        },
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
    };
    // Record how the binary was produced: without it a replay cannot rebuild
    // the same bytes, and the capsule would not be self-contained.
    capsule.build.argv = pipeline.build_argv();
    capsule.seal()?;
    capsule.validate()?;

    let capsule_path = out
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| sol.root.join("capsule.json"));
    std::fs::write(&capsule_path, capsule.to_json_pretty()?)?;

    let hints = explain::explain(&capsule, &rules);

    let report = MeasureReport {
        capsule_path: capsule_path.display().to_string(),
        capsule_id: capsule.capsule_id.clone().unwrap_or_default(),
        verdict: capsule.verdict,
        ranked_metric: format!("{:?}", sol.problem.meta.ranked_metric).to_lowercase(),
        runs,
        metrics: capsule
            .metrics
            .iter()
            .map(|m| MetricSummary {
                name: m.name.clone(),
                value: m.value,
                unit: m.unit.clone(),
                kind: m.kind,
                method: m.method,
                uncertainty: m.uncertainty.as_ref().map(|u| u.value),
            })
            .collect(),
        hints: hints.clone(),
    };

    cmd::emit(
        format,
        || {
            println!("capsule {}", &report.capsule_id[..16.min(report.capsule_id.len())]);
            println!("  problem   {}", sol.problem.meta.id);
            println!("  ranked on {}", report.ranked_metric);
            println!("  runs      {}", report.runs);
            println!();
            println!("  {:<18} {:>14}  {:<10} {:<12} {}", "metric", "value", "unit", "kind", "method");
            for m in &report.metrics {
                let value = if m.value >= 1e6 {
                    format!("{:.3e}", m.value)
                } else {
                    format!("{:.0}", m.value)
                };
                let spread = m
                    .uncertainty
                    .map(|hw| format!("  +/- {hw:.3e}"))
                    .unwrap_or_default();
                println!(
                    "  {:<18} {:>14}  {:<10} {:<12} {}{spread}",
                    m.name,
                    value,
                    m.unit,
                    format!("{:?}", m.kind).to_lowercase(),
                    format!("{:?}", m.method).to_lowercase(),
                );
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
