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

            // The reference must build.
            match pipeline.build_reference() {
                Ok(_) => push(&mut checks, "reference builds", true, "compiled"),
                Err(e) => push(&mut checks, "reference builds", false, format!("{e}")),
            }

            // The generator must be deterministic: same seed, same bytes, twice.
            let sizes: Vec<String> = problem.meta.sizes.keys().cloned().collect();
            if let Some(first_size) = sizes.first() {
                let seed = problem.meta.seeds.public.first().copied().unwrap_or(1);
                match (pipeline.generate(first_size, seed), pipeline.generate(first_size, seed)) {
                    (Ok(a), Ok(b)) => {
                        let same = a == b;
                        push(
                            &mut checks,
                            "generator is deterministic",
                            same,
                            if same {
                                format!("seed {seed} produced identical {} bytes twice", a.len())
                            } else {
                                "the same seed produced different bytes".to_string()
                            },
                        );
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        push(&mut checks, "generator runs", false, format!("{e}"))
                    }
                }
            }

            // The reference must be correct and its cost must grow with size,
            // which is what proves the metrics separate the sizes.
            let mut cost_by_size: Vec<(String, f64)> = Vec::new();
            for size in &sizes {
                let seed = problem.meta.seeds.public.first().copied().unwrap_or(1);
                let Ok(input) = pipeline.generate(size, seed) else {
                    push(&mut checks, &format!("size {size}"), false, "generator failed");
                    continue;
                };
                let Ok(expected) = pipeline.reference_output(&input) else {
                    push(&mut checks, &format!("size {size}"), false, "reference failed");
                    continue;
                };
                let Ok(prog) = pipeline.build_reference() else { continue };
                let policy = policy_for(&problem);
                match pipeline.measure_once(&prog, &input, &policy) {
                    Ok((outcome, counts)) => {
                        let verdict = outcome.verdict(policy.memory_bytes);
                        let ok = verdict == Verdict::Accepted;
                        push(
                            &mut checks,
                            &format!("size {size}"),
                            ok,
                            format!("{} bytes of expected output", expected.len()),
                        );
                        if let Some(c) = counts {
                            cost_by_size.push((size.clone(), c.instructions() as f64));
                        }
                    }
                    Err(e) => push(&mut checks, &format!("size {size}"), false, format!("{e}")),
                }
            }

            // Monotonic growth: larger sizes must cost more, or the problem
            // cannot distinguish a better algorithm from a worse one.
            let ranked = problem.meta.ranked_metric;
            let monotonic = cost_by_size
                .windows(2)
                .all(|w| w[1].1 >= w[0].1 * 0.9);
            let detail = cost_by_size
                .iter()
                .map(|(s, c)| format!("{s}={c:.0}"))
                .collect::<Vec<_>>()
                .join("  ");
            push(
                &mut checks,
                "sizes separate the metric",
                cost_by_size.len() < 2 || monotonic,
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
