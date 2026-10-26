//! `ccx test` — run the solution against the seeded tests and report verdicts.
//!
//! Correctness is separate from speed, and both are separate from explanation.
//! A submission that is wrong never reaches measurement, because measuring a
//! wrong answer is meaningless.
//!
//! Two seed sets matter. Public seeds ship with the problem so people can
//! practise; hidden seeds are generated fresh so an answer cannot be hard-coded.
//! Both are run, and the verdict records which.

use crate::cmd::{self, Fixable};
use crate::context::{policy_for, Solution};
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::Verdict;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub size: String,
    pub seed: u64,
    pub seed_set: String,
    pub verdict: Verdict,
    pub exit_status: i32,
    pub signal: i32,
    pub wall_ns: u64,
    pub peak_rss_bytes: u64,
    /// First differing line, when the answer was wrong. Truncated for display.
    pub first_difference: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub problem: String,
    pub sizes_tested: Vec<String>,
    pub cases: Vec<CaseResult>,
    pub passed: usize,
    pub failed: usize,
    pub overall: Verdict,
}

/// Compare two answers, returning a short description of the first difference.
fn first_difference(expected: &[u8], actual: &[u8]) -> Option<String> {
    if expected == actual {
        return None;
    }
    let exp = String::from_utf8_lossy(expected);
    let act = String::from_utf8_lossy(actual);
    let exp_lines: Vec<&str> = exp.lines().collect();
    let act_lines: Vec<&str> = act.lines().collect();

    for (i, (e, a)) in exp_lines.iter().zip(act_lines.iter()).enumerate() {
        if e != a {
            return Some(format!(
                "line {}: expected {:?}, got {:?}",
                i + 1,
                truncate(e),
                truncate(a)
            ));
        }
    }
    // No line differed, so it is a length or trailing-byte difference.
    Some(format!(
        "output length differs: expected {} lines, got {}",
        exp_lines.len(),
        act_lines.len()
    ))
}

fn truncate(s: &str) -> String {
    if s.len() > 60 {
        format!("{}…", &s[..60])
    } else {
        s.to_string()
    }
}

pub fn run(format: Format, dir: &std::path::Path, include_hidden: bool) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let pipeline = Pipeline::new(&sol)?;
    let policy = policy_for(&sol.problem);

    let sizes: Vec<String> = sol.problem.meta.sizes.keys().cloned().collect();
    // Build the solution, generator, and reference once. A container start per
    // case would dominate the run and prove nothing about the submission.
    let program = pipeline.build_solution()?;
    let generator = pipeline.build_generator()?;
    let reference = pipeline.build_reference()?;

    let mut cases = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;

    // Public seeds always run. Hidden seeds run too when asked, which is what
    // makes hard-coding an answer fail.
    let mut seed_sets: Vec<(&str, Vec<u64>)> =
        vec![("public", sol.problem.meta.seeds.public.clone())];
    if include_hidden {
        seed_sets.push(("hidden", sol.problem.meta.seeds.hidden.clone()));
    }

    for (set_name, seeds) in seed_sets {
        for size in &sizes {
            for &seed in &seeds {
                let input = pipeline.generate_with(&generator, size, seed)?;
                let expected = pipeline.reference_output_with(&reference, &input)?;
                let outcome = pipeline.run_program(&program, &input, &policy, false)?;
                let verdict = outcome.verdict(policy.memory_bytes);

                let diff = if verdict == Verdict::Accepted {
                    first_difference(&expected, outcome.stdout.as_bytes())
                } else {
                    None
                };

                let final_verdict = match (&diff, verdict) {
                    (Some(_), Verdict::Accepted) => Verdict::WrongAnswer,
                    _ => verdict,
                };

                match final_verdict {
                    Verdict::Accepted => passed += 1,
                    _ => failed += 1,
                }

                cases.push(CaseResult {
                    size: size.clone(),
                    seed,
                    seed_set: set_name.to_string(),
                    verdict: final_verdict,
                    exit_status: outcome.exit_status,
                    signal: outcome.signal,
                    wall_ns: outcome.wall_ns,
                    peak_rss_bytes: outcome.peak_rss_bytes,
                    first_difference: diff,
                });
            }
        }
    }

    let overall = if failed == 0 {
        Verdict::Accepted
    } else {
        // The worst verdict wins, so a timeout is never reported as merely
        // "wrong answer".
        cases
            .iter()
            .map(|c| c.verdict)
            .max_by_key(|v| severity(*v))
            .unwrap_or(Verdict::WrongAnswer)
    };

    let report = TestReport {
        problem: sol.problem.meta.id.clone(),
        sizes_tested: sizes,
        cases,
        passed,
        failed,
        overall,
    };

    cmd::emit(
        format,
        || {
            println!("{}: {passed} passed, {failed} failed", report.problem);
            for c in &report.cases {
                let mark = if c.verdict == Verdict::Accepted { "ok  " } else { "FAIL" };
                let mut line = format!(
                    "  {mark} {:>8}  seed {:>6} ({})  {}",
                    c.size,
                    c.seed,
                    c.seed_set,
                    c.verdict
                );
                if let Some(d) = &c.first_difference {
                    line.push_str(&format!("\n        {d}"));
                }
                println!("{line}");
            }
        },
        &report,
    )?;

    if report.overall != Verdict::Accepted {
        return Err(Fixable::new(format!(
            "{} test(s) failed",
            report.failed
        ))
        .fix("run `ccx test --format json` for the full case list")
        .fix("the first difference in each failing case is shown above")
        .into_error());
    }
    Ok(())
}

/// Ordering used to pick the most severe verdict. A timeout or memory-limit
/// result outranks a wrong answer, because it means the submission never
/// produced an answer at all.
fn severity(v: Verdict) -> u8 {
    match v {
        Verdict::Accepted => 0,
        Verdict::WrongAnswer => 1,
        Verdict::BuildError => 2,
        Verdict::RuntimeError => 3,
        Verdict::RejectedUnsafe => 4,
        Verdict::MemoryLimit => 5,
        Verdict::TimeLimit => 6,
    }
}
