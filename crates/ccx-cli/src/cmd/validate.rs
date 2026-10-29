//! `ccx validate` — check a problem folder end to end.
//!
//! A problem is a folder of files that must satisfy structural rules, and then
//! a set of executable claims: the reference solution must be correct, the
//! sizes must actually separate the metrics, and the generators must be
//! deterministic. A problem that fails any of these would produce meaningless
//! results, so `validate` runs them all.

use crate::cmd::{self, Fixable};
use crate::context::policy_for;
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::Verdict;
use ccx_core::problem::Judge;
use ccx_core::problem::{Problem, RankedMetric};
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ValidateReport {
    pub problem: String,
    pub checks: Vec<Check>,
    pub ok: bool,
}

fn push(checks: &mut Vec<Check>, name: &str, ok: bool, detail: impl Into<String>) {
    checks.push(Check {
        name: name.into(),
        ok,
        detail: detail.into(),
    });
}

pub fn run(format: Format, path: &Path) -> anyhow::Result<()> {
    let problem = Problem::load(path).map_err(|e| {
        Fixable::new(format!("{} is not a valid problem: {e}", path.display()))
            .fix("a problem folder needs problem.json, statement.md, generator.c, reference.cpp, editorial.md, hints.md")
            .into_error()
    })?;

    let mut checks = Vec::new();

    // Metadata rules: internal consistency before anything executes.
    match problem.check_metadata() {
        Ok(()) => push(&mut checks, "metadata", true, "consistent"),
        Err(e) => push(&mut checks, "metadata", false, e.to_string()),
    }

    // Every required file present.
    for rel in problem.required_files() {
        let present = problem.root.join(&rel).is_file();
        push(
            &mut checks,
            &format!("file {rel}"),
            present,
            if present { "present" } else { "missing" },
        );
    }

    // Executable checks need Docker; skip them clearly rather than silently.
    let sol = match crate::context::Solution::for_problem(&problem) {
        Ok(s) => Some(s),
        Err(e) => {
            push(&mut checks, "engine", false, format!("{e}"));
            None
        }
    };

    if let Some(sol) = sol {
        let pipeline = match Pipeline::new(&sol) {
            Ok(p) => Some(p),
            Err(e) => {
                push(&mut checks, "docker", false, format!("{e}"));
                None
            }
        };

        if let Some(pipeline) = pipeline {
            push(&mut checks, "docker", true, "available");
            let validation_seeds = problem
                .meta
                .seeds
                .public
                .iter()
                .chain(problem.meta.seeds.hidden.iter())
                .copied()
                .collect::<Vec<_>>();
            let mut sizes: Vec<String> = problem.meta.sizes.keys().cloned().collect();
            sizes.sort_by_key(|size| problem.meta.sizes.get(size).copied().unwrap_or_default());
            push(
                &mut checks,
                "validation cases available",
                !sizes.is_empty() && !validation_seeds.is_empty(),
                format!(
                    "{} sizes and {} public or hidden seeds",
                    sizes.len(),
                    validation_seeds.len()
                ),
            );

            // The reference must build.
            let reference = match pipeline.build_reference() {
                Ok(reference) => {
                    push(&mut checks, "reference builds", true, "compiled");
                    Some(reference)
                }
                Err(e) => {
                    push(&mut checks, "reference builds", false, format!("{e}"));
                    None
                }
            };

            let judge = problem.meta.judge;
            let checker = if judge == Judge::Checker {
                match pipeline.build_checker() {
                    Ok(checker) => Some(checker),
                    Err(e) => {
                        push(&mut checks, "checker builds", false, format!("{e}"));
                        None
                    }
                }
            } else {
                None
            };

            match pipeline.build_problem_cpp("brute_force.cpp", "brute_force") {
                Ok(brute) => {
                    if let Some(size) = sizes.first() {
                        let result = (|| -> anyhow::Result<String> {
                            let reference = reference.as_ref().ok_or_else(|| {
                                anyhow::anyhow!("reference solution did not build")
                            })?;
                            for seed in &validation_seeds {
                                let input = pipeline.generate(size, *seed)?;
                                let expected = pipeline.reference_output_with(reference, &input)?;
                                let actual = pipeline.run_program(
                                    &brute,
                                    &input,
                                    &policy_for(&problem),
                                    false,
                                )?;
                                if actual.verdict(problem.meta.limits.memory_bytes)
                                    != Verdict::Accepted
                                {
                                    anyhow::bail!("brute force failed on size {size}, seed {seed}");
                                }
                                let matches = if let Some(checker) = &checker {
                                    pipeline.check_output(
                                        checker,
                                        &input,
                                        &expected,
                                        actual.stdout.as_bytes(),
                                        &policy_for(&problem),
                                    )?
                                } else {
                                    actual.stdout.as_bytes() == expected.as_slice()
                                };
                                if !matches {
                                    anyhow::bail!(
                                        "brute-force output differs on size {size}, seed {seed}"
                                    );
                                }
                            }
                            Ok(format!(
                                "size {size}; {} public and hidden seeds",
                                validation_seeds.len()
                            ))
                        })();
                        match result {
                            Ok(detail) => push(&mut checks, "brute force agrees", true, detail),
                            Err(e) => {
                                push(&mut checks, "brute force agrees", false, format!("{e}"))
                            }
                        }
                    }
                }
                Err(e) => push(&mut checks, "brute force builds", false, format!("{e}")),
            }

            // The generator must be deterministic: same seed, same bytes, twice.
            let generator_check = (|| -> anyhow::Result<usize> {
                let mut checked = 0;
                for size in &sizes {
                    for seed in &validation_seeds {
                        let first = pipeline.generate(size, *seed)?;
                        let second = pipeline.generate(size, *seed)?;
                        if first != second {
                            anyhow::bail!("size {size}, seed {seed} produced different bytes");
                        }
                        checked += 1;
                    }
                }
                Ok(checked)
            })();
            match generator_check {
                Ok(checked) => push(
                    &mut checks,
                    "generator is deterministic",
                    true,
                    format!("identical bytes across {checked} size and seed combinations"),
                ),
                Err(e) => push(
                    &mut checks,
                    "generator is deterministic",
                    false,
                    format!("{e}"),
                ),
            };
            let mut variant_programs = std::collections::BTreeMap::new();
            for size in &sizes {
                let variant_source = format!("solutions/{size}.cpp");
                match pipeline.build_problem_cpp(&variant_source, &format!("variant_{size}")) {
                    Ok(variant) => {
                        variant_programs.insert(size.clone(), variant.clone());
                        let result = (|| -> anyhow::Result<String> {
                            let reference = reference.as_ref().ok_or_else(|| {
                                anyhow::anyhow!("reference solution did not build")
                            })?;
                            for seed in &validation_seeds {
                                let input = pipeline.generate(size, *seed)?;
                                let expected = pipeline.reference_output_with(reference, &input)?;
                                let actual = pipeline.run_program(
                                    &variant,
                                    &input,
                                    &policy_for(&problem),
                                    false,
                                )?;
                                if actual.verdict(problem.meta.limits.memory_bytes)
                                    != Verdict::Accepted
                                {
                                    anyhow::bail!("solution failed on size {size}, seed {seed}");
                                }
                                let matches = if let Some(checker) = &checker {
                                    pipeline.check_output(
                                        checker,
                                        &input,
                                        &expected,
                                        actual.stdout.as_bytes(),
                                        &policy_for(&problem),
                                    )?
                                } else {
                                    actual.stdout.as_bytes() == expected.as_slice()
                                };
                                if !matches {
                                    anyhow::bail!(
                                        "solution differs from the reference on size {size}, seed {seed}"
                                    );
                                }
                            }
                            Ok(format!(
                                "{} public and hidden seeds",
                                validation_seeds.len()
                            ))
                        })();
                        match result {
                            Ok(detail) => {
                                push(&mut checks, &format!("solution {size}"), true, detail)
                            }
                            Err(e) => push(
                                &mut checks,
                                &format!("solution {size}"),
                                false,
                                format!("{e}"),
                            ),
                        }
                    }
                    Err(e) => push(
                        &mut checks,
                        &format!("solution {size} builds"),
                        false,
                        format!("{e}"),
                    ),
                }
            }

            if let Some(size) = sizes.first() {
                let common_input = problem
                    .meta
                    .seeds
                    .public
                    .first()
                    .copied()
                    .ok_or_else(|| anyhow::anyhow!("no public seeds available"))
                    .and_then(|seed| pipeline.generate(size, seed));
                match common_input {
                    Ok(input) => {
                        let mut instruction_counts = std::collections::BTreeMap::new();
                        for variant_name in &sizes {
                            let result = (|| -> anyhow::Result<(u64, bool, String)> {
                                let variant =
                                    variant_programs.get(variant_name).ok_or_else(|| {
                                        anyhow::anyhow!("solution variant did not build")
                                    })?;
                                let policy = policy_for(&problem);
                                let mut counts = Vec::new();
                                let mut allocations = Vec::new();
                                for _ in 0..3 {
                                    let (outcome, measurement) =
                                        pipeline.measure_once(variant, &input, &policy)?;
                                    if outcome.verdict(policy.memory_bytes) != Verdict::Accepted {
                                        anyhow::bail!("measurement process was not accepted");
                                    }
                                    counts.push(measurement.ok_or_else(|| {
                                        anyhow::anyhow!("Cachegrind counters missing")
                                    })?);
                                    allocations.push(outcome.allocations);
                                }
                                let counts_stable = counts.iter().all(|count| *count == counts[0]);
                                let allocations_present = allocations.iter().all(Option::is_some);
                                let allocations_stable =
                                    allocations.iter().all(|value| *value == allocations[0]);
                                let mut instructions = counts
                                    .iter()
                                    .map(|measurement| measurement.instructions())
                                    .collect::<Vec<_>>();
                                instructions.sort_unstable();
                                let median_instructions = instructions[instructions.len() / 2];
                                let detail = format!(
                                    "3 runs; instructions {}..{}; Cachegrind {}; allocations {}",
                                    instructions[0],
                                    instructions[instructions.len() - 1],
                                    if counts_stable { "stable" } else { "varying" },
                                    if !allocations_present {
                                        "missing"
                                    } else if allocations_stable {
                                        "stable"
                                    } else {
                                        "varying"
                                    }
                                );
                                Ok((
                                    median_instructions,
                                    counts_stable && allocations_present && allocations_stable,
                                    detail,
                                ))
                            })();
                            match result {
                                Ok((count, stable, detail)) => {
                                    instruction_counts.insert(variant_name.clone(), count);
                                    push(
                                        &mut checks,
                                        &format!("measurement stable {variant_name}"),
                                        stable,
                                        detail,
                                    );
                                }
                                Err(e) => push(
                                    &mut checks,
                                    &format!("measurement stable {variant_name}"),
                                    false,
                                    format!("{e}"),
                                ),
                            }
                        }
                        if let (Some(naive), Some(good)) = (
                            instruction_counts.get("naive"),
                            instruction_counts.get("good"),
                        ) {
                            push(
                                &mut checks,
                                "good beats naive",
                                good < naive,
                                format!("same input: naive={naive}, good={good} instructions"),
                            );
                        } else {
                            push(
                                &mut checks,
                                "good beats naive",
                                false,
                                "could not measure both naive and good variants".to_string(),
                            );
                        }
                    }
                    Err(e) => push(
                        &mut checks,
                        "good beats naive",
                        false,
                        format!("could not generate common input: {e}"),
                    ),
                }
            }

            // The reference must be correct and its cost must grow with size,
            // which is what proves the metrics separate the sizes.
            let mut cost_by_size: Vec<(String, f64)> = Vec::new();
            let mut reference_measurements_complete = true;
            if let Some(reference) = reference.as_ref() {
                let policy = policy_for(&problem);
                for size in &sizes {
                    let mut costs = Vec::new();
                    for seed in &validation_seeds {
                        let result = (|| -> anyhow::Result<u64> {
                            let input = pipeline.generate(size, *seed)?;
                            let expected = pipeline.reference_output_with(reference, &input)?;
                            let (outcome, counts) =
                                pipeline.measure_once(reference, &input, &policy)?;
                            if outcome.verdict(policy.memory_bytes) != Verdict::Accepted {
                                anyhow::bail!("reference failed on size {size}, seed {seed}");
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
                                anyhow::bail!(
                                    "reference output changed on size {size}, seed {seed}"
                                );
                            }
                            counts
                                .map(|counts| counts.instructions())
                                .ok_or_else(|| anyhow::anyhow!("Cachegrind counters missing"))
                        })();
                        match result {
                            Ok(instructions) => {
                                costs.push(instructions);
                                push(
                                    &mut checks,
                                    &format!("reference {size} seed {seed}"),
                                    true,
                                    format!("{instructions} instructions"),
                                );
                            }
                            Err(e) => {
                                reference_measurements_complete = false;
                                push(
                                    &mut checks,
                                    &format!("reference {size} seed {seed}"),
                                    false,
                                    format!("{e}"),
                                );
                            }
                        }
                    }
                    if costs.len() == validation_seeds.len() {
                        costs.sort_unstable();
                        cost_by_size.push((size.clone(), costs[costs.len() / 2] as f64));
                    } else {
                        reference_measurements_complete = false;
                    }
                }
            }

            // Monotonic growth: larger sizes must cost more, or the problem
            // cannot distinguish a better algorithm from a worse one.
            let ranked = problem.meta.ranked_metric;
            let monotonic = cost_by_size.len() == sizes.len()
                && cost_by_size.windows(2).all(|w| w[1].1 > w[0].1 * 0.9);
            let detail = cost_by_size
                .iter()
                .map(|(s, c)| format!("{s}={c:.0}"))
                .collect::<Vec<_>>()
                .join("  ");
            push(
                &mut checks,
                "sizes separate the metric",
                reference_measurements_complete && monotonic,
                match ranked {
                    RankedMetric::Instructions => format!("instructions: {detail}"),
                    other => format!("{other:?}: {detail}"),
                },
            );
        }
    }

    let ok = checks.iter().all(|c| c.ok);
    let report = ValidateReport {
        problem: problem.meta.id.clone(),
        checks,
        ok,
    };

    cmd::emit(
        format,
        || {
            println!("validating {}", report.problem);
            for c in &report.checks {
                let mark = if c.ok { "ok  " } else { "FAIL" };
                println!("  {mark} {:<28} {}", c.name, c.detail);
            }
            println!();
            println!("  {}", if report.ok { "valid" } else { "INVALID" });
        },
        &report,
    )?;

    if !report.ok {
        let failed = report
            .checks
            .iter()
            .filter(|c| !c.ok)
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(Fixable::new(format!("problem validation failed: {failed}"))
            .fix("fix each FAIL line above")
            .into_error());
    }
    Ok(())
}
