//! `ccx run` — build and run once, showing the program's output.
//!
//! The debugging loop: build, feed one generated input, show what came out and
//! what was expected. No measurement, no capsule — just "is this working".

use crate::cmd::{self, Fixable};
use crate::context::{policy_for, Solution};
use crate::pipeline::Pipeline;
use crate::Format;
use ccx_core::capsule::Verdict;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RunReport {
    pub size: String,
    pub seed: u64,
    pub verdict: Verdict,
    pub exit_status: i32,
    pub signal: i32,
    pub wall_ns: u64,
    pub peak_rss_bytes: u64,
    /// First 4 KiB of the program's output, for eyeballing.
    pub stdout_preview: String,
    pub stderr: String,
    pub matches_reference: Option<bool>,
}

pub fn run(
    format: Format,
    dir: &std::path::Path,
    size: &str,
    seed: u64,
    show_expected: bool,
) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let pipeline = Pipeline::new(&sol)?;
    let policy = policy_for(&sol.problem);

    let program = pipeline.build_solution()?;
    let input = pipeline.generate(size, seed)?;
    let outcome = pipeline.run_program(&program, &input, &policy, false)?;
    let verdict = outcome.verdict(policy.memory_bytes);

    let matches = if verdict == Verdict::Accepted {
        let expected = pipeline.reference_output(&input)?;
        Some(outcome.stdout.as_bytes() == expected.as_slice())
    } else {
        None
    };

    let report = RunReport {
        size: size.to_string(),
        seed,
        verdict,
        exit_status: outcome.exit_status,
        signal: outcome.signal,
        wall_ns: outcome.wall_ns,
        peak_rss_bytes: outcome.peak_rss_bytes,
        stdout_preview: outcome.stdout.chars().take(4096).collect(),
        stderr: outcome.stderr.chars().take(2048).collect(),
        matches_reference: matches,
    };

    cmd::emit(
        format,
        || {
            println!("{} seed {seed} size {size}: {verdict}", sol.problem.meta.id);
            println!(
                "  exit {} signal {}  {:.3} ms  {:.1} MiB peak",
                report.exit_status,
                report.signal,
                report.wall_ns as f64 / 1e6,
                report.peak_rss_bytes as f64 / (1024.0 * 1024.0),
            );
            if let Some(true) = report.matches_reference {
                println!("  output matches the reference");
            } else if let Some(false) = report.matches_reference {
                println!("  output DOES NOT match the reference");
            }
            if !report.stdout_preview.is_empty() {
                println!();
                println!("  stdout:");
                for line in report.stdout_preview.lines().take(20) {
                    println!("    {line}");
                }
            }
            if !report.stderr.is_empty() {
                println!();
                println!("  stderr:");
                for line in report.stderr.lines().take(20) {
                    println!("    {line}");
                }
            }
        },
        &report,
    )?;

    if show_expected {
        let expected = pipeline.reference_output(&input)?;
        println!();
        println!("  expected:");
        for line in String::from_utf8_lossy(&expected).lines().take(20) {
            println!("    {line}");
        }
    }

    if verdict != Verdict::Accepted {
        return Err(Fixable::new(format!("the run did not complete cleanly: {verdict:?}"))
            .fix("see the stderr above")
            .into_error());
    }
    Ok(())
}
