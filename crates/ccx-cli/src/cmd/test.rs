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
use ccx_core::engine::{ResourceUsage, RunOutcome};
use ccx_core::problem::Judge;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

const MAX_CASES: u64 = 1_000_000;
const MAX_JOBS: usize = 16;
const BATCH_CASES_PER_CONTAINER: u64 = 10000;
const STORED_GENERATED_PASSES: usize = 20;
const STORED_GENERATED_FAILURES: usize = 1_000;

#[derive(Debug, Clone, Serialize)]
pub struct CaseResult {
    pub sequence: Option<u64>,
    pub size: String,
    pub seed: u64,
    pub seed_set: String,
    pub verdict: Verdict,
    pub exit_status: i32,
    pub signal: i32,
    pub wall_ns: u64,
    pub peak_rss_bytes: u64,
    pub resources: Option<ResourceUsage>,
    /// First differing line, when the answer was wrong. Truncated for display.
    pub first_difference: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TestReport {
    pub problem: String,
    pub sizes_tested: Vec<String>,
    pub generated_size: Option<String>,
    pub generated_cases: u64,
    pub generated_cases_requested: u64,
    pub complete: bool,
    pub case_details_omitted: u64,
    pub case_log: Option<String>,
    pub cases: Vec<CaseResult>,
    pub total_cases: u64,
    pub passed: usize,
    pub failed: usize,
    pub overall: Verdict,
}

#[derive(Serialize)]
struct CaseLogHeader<'a> {
    record_type: &'static str,
    problem: &'a str,
    generated_size: Option<&'a str>,
    generated_cases: u64,
    first_seed: u64,
}

#[derive(Serialize)]
struct CaseLogEntry<'a> {
    record_type: &'static str,
    case_index: u64,
    case: &'a CaseResult,
}

#[derive(Serialize)]
struct CaseLogSummary {
    record_type: &'static str,
    complete: bool,
    total_cases: u64,
    passed: usize,
    failed: usize,
}

#[derive(Debug, Deserialize)]
struct BatchCaseRecord {
    seed: u64,
    stage_error: Option<String>,
    generator: Option<BatchProcessRecord>,
    reference: Option<BatchProcessRecord>,
    solution: Option<BatchProcessRecord>,
    checker: Option<BatchProcessRecord>,
    checker_accepted: bool,
    expected_bytes: u64,
    actual_bytes: u64,
    outputs_equal: bool,
    difference_offset: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct BatchProcessRecord {
    exit_status: i32,
    signal: i32,
    timed_out: bool,
    output_truncated: bool,
    peak_rss_bytes: u64,
    wall_ns: u64,
    stdout_bytes: u64,
    stderr_bytes: u64,
    user_cpu_ns: Option<u64>,
    system_cpu_ns: Option<u64>,
    minor_page_faults: Option<u64>,
    major_page_faults: Option<u64>,
    voluntary_context_switches: Option<u64>,
    involuntary_context_switches: Option<u64>,
    block_input_ops: Option<u64>,
    block_output_ops: Option<u64>,
}

impl BatchProcessRecord {
    fn resources(&self) -> Option<ResourceUsage> {
        let present = self.user_cpu_ns.is_some()
            || self.system_cpu_ns.is_some()
            || self.minor_page_faults.is_some()
            || self.major_page_faults.is_some()
            || self.voluntary_context_switches.is_some()
            || self.involuntary_context_switches.is_some()
            || self.block_input_ops.is_some()
            || self.block_output_ops.is_some();
        present.then_some(ResourceUsage {
            user_cpu_ns: self.user_cpu_ns,
            system_cpu_ns: self.system_cpu_ns,
            minor_page_faults: self.minor_page_faults,
            major_page_faults: self.major_page_faults,
            voluntary_context_switches: self.voluntary_context_switches,
            involuntary_context_switches: self.involuntary_context_switches,
            block_input_ops: self.block_input_ops,
            block_output_ops: self.block_output_ops,
        })
    }

    fn outcome(&self) -> RunOutcome {
        RunOutcome {
            exit_status: self.exit_status,
            signal: self.signal,
            timed_out: self.timed_out,
            output_truncated: self.output_truncated,
            artifact_too_large: false,
            peak_rss_bytes: self.peak_rss_bytes,
            wall_ns: self.wall_ns,
            stdout_bytes: self.stdout_bytes,
            stderr_bytes: self.stderr_bytes,
            stdout: String::new(),
            stderr: String::new(),
            resources: self.resources(),
            allocations: None,
            hardware: None,
        }
    }
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
    s.chars().take(60).collect::<String>() + if s.chars().count() > 60 { "…" } else { "" }
}

fn validate_stress_request(
    generated_cases: u64,
    first_seed: u64,
    jobs: usize,
) -> anyhow::Result<()> {
    if generated_cases > MAX_CASES {
        return Err(Fixable::new(format!(
            "generated case count {generated_cases} exceeds the supported limit"
        ))
        .fix(format!("choose between 0 and {MAX_CASES} generated cases"))
        .into_error());
    }
    if !(1..=MAX_JOBS).contains(&jobs) {
        return Err(Fixable::new(format!(
            "worker count {jobs} is outside the supported range"
        ))
        .fix(format!("choose between 1 and {MAX_JOBS} workers"))
        .into_error());
    }
    if generated_cases > 0 && first_seed.checked_add(generated_cases - 1).is_none() {
        return Err(Fixable::new("generated seed range overflows u64")
            .fix("choose a lower --seed or a smaller --cases count")
            .into_error());
    }
    Ok(())
}

pub fn run(
    format: Format,
    dir: &Path,
    include_hidden: bool,
    generated_cases: u64,
    first_seed: u64,
    requested_size: Option<&str>,
    jobs: usize,
    case_log_path: Option<&Path>,
) -> anyhow::Result<()> {
    validate_stress_request(generated_cases, first_seed, jobs)?;

    let sol = Solution::load(dir)?;
    let pipeline = Pipeline::new(&sol)?;
    let mut policy = policy_for(&sol.problem);
    policy.cpuset = None;

    let sizes: Vec<String> = sol.problem.meta.sizes.keys().cloned().collect();
    // Build the solution, generator, and reference once. A container start per
    // case would dominate the run and prove nothing about the submission.
    let program = pipeline.build_solution()?;
    let generator = pipeline.build_generator()?;
    let reference = pipeline.build_reference()?;
    let checker = if sol.problem.meta.judge == Judge::Checker {
        Some(pipeline.build_checker()?)
    } else {
        None
    };

    let generated_size = if generated_cases > 0 {
        Some(match requested_size {
            Some(size) if sol.problem.meta.sizes.contains_key(size) => size.to_string(),
            Some(size) => {
                return Err(Fixable::new(format!("problem has no size named {size:?}"))
                    .fix(format!(
                        "available sizes: {}",
                        sol.problem
                            .meta
                            .sizes
                            .iter()
                            .map(|(name, scale)| format!("{name} ({scale})"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .into_error());
            }
            None => sol
                .problem
                .meta
                .sizes
                .iter()
                .max_by_key(|(_, scale)| *scale)
                .map(|(name, _)| name.clone())
                .ok_or_else(|| anyhow::anyhow!("problem has no generated input sizes"))?,
        })
    } else {
        None
    };

    let mut case_log = case_log_path
        .map(|path| File::create(path).map(BufWriter::new))
        .transpose()?;
    if let Some(log) = case_log.as_mut() {
        serde_json::to_writer(
            &mut *log,
            &CaseLogHeader {
                record_type: "header",
                problem: &sol.problem.meta.id,
                generated_size: generated_size.as_deref(),
                generated_cases,
                first_seed,
            },
        )?;
        log.write_all(b"\n")?;
    }

    let mut cases = Vec::new();
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut worst_verdict = Verdict::Accepted;
    let mut generated_completed = 0u64;

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
                let result = execute_case(
                    &pipeline,
                    &generator,
                    &reference,
                    &program,
                    checker.as_ref(),
                    &policy,
                    size,
                    seed,
                    set_name,
                    None,
                )?;
                let final_verdict = result.verdict;

                match final_verdict {
                    Verdict::Accepted => passed += 1,
                    _ => failed += 1,
                }
                if severity(final_verdict) > severity(worst_verdict) {
                    worst_verdict = final_verdict;
                }
                if let Some(log) = case_log.as_mut() {
                    write_case_log(log, cases.len() as u64, &result)?;
                }
                cases.push(result);
            }
        }
    }

    let baseline_case_count = cases.len();
    let mut case_details_omitted = 0u64;
    let mut generated_attempted = false;
    if generated_cases > 0 && failed == 0 {
        generated_attempted = true;
        let size = generated_size.as_deref().unwrap_or_default();
        let next_case = AtomicU64::new(0);
        let stopped = AtomicBool::new(false);
        let (sender, receiver) = mpsc::sync_channel::<(u64, anyhow::Result<CaseResult>)>(jobs * 2);
        let batch_cases_per_worker =
            BATCH_CASES_PER_CONTAINER.min(generated_cases.div_ceil(jobs as u64));
        let mut generated_passes = 0u64;
        let mut generated_failures = 0u64;
        let mut generated_omitted = 0u64;
        let mut first_error = None;
        let stress_started = Instant::now();
        thread::scope(|scope| {
            for _ in 0..jobs {
                let sender = sender.clone();
                let pipeline = &pipeline;
                let generator = &generator;
                let reference = &reference;
                let program = &program;
                let checker = checker.as_ref();
                let policy = &policy;
                let next_case = &next_case;
                let stopped = &stopped;
                scope.spawn(move || loop {
                    if stopped.load(Ordering::Acquire) {
                        break;
                    }
                    let index = next_case.fetch_add(batch_cases_per_worker, Ordering::Relaxed);
                    if index >= generated_cases {
                        break;
                    }
                    let count = batch_cases_per_worker.min(generated_cases - index);
                    let first_seed_in_batch = first_seed + index;
                    let execution = pipeline.run_batch(
                        generator,
                        reference,
                        program,
                        checker,
                        policy,
                        size,
                        first_seed_in_batch,
                        count,
                    );
                    let execution = match execution {
                        Ok(execution) => execution,
                        Err(error) => {
                            stopped.store(true, Ordering::Release);
                            let _ = sender.send((
                                index,
                                Err(anyhow::anyhow!(
                                    "seed {first_seed_in_batch} at size {size}: {error:#}"
                                )),
                            ));
                            break;
                        }
                    };
                    let records = match parse_batch_records(&execution.stdout) {
                        Ok(records) => records,
                        Err(error) => {
                            stopped.store(true, Ordering::Release);
                            let _ = sender.send((
                                index,
                                Err(anyhow::anyhow!(
                                    "batch for seed {first_seed_in_batch} at size {size} failed: {error:#}; {}",
                                    execution.stderr
                                )),
                            ));
                            break;
                        }
                    };
                    let record_count = records.len() as u64;
                    if record_count > count {
                        stopped.store(true, Ordering::Release);
                        let _ = sender.send((
                            index,
                            Err(anyhow::anyhow!(
                                "batch runner returned {record_count} records for {count} requested cases"
                            )),
                        ));
                        break;
                    }
                    let mut failed = false;
                    for (offset, record) in records.into_iter().enumerate() {
                        let case_index = index + offset as u64;
                        let expected_seed = first_seed + case_index;
                        let result = case_from_batch(
                            record,
                            size,
                            expected_seed,
                            case_index,
                            policy,
                            checker.is_some(),
                        )
                        .map_err(|error| {
                            anyhow::anyhow!("seed {expected_seed} at size {size}: {error:#}")
                        });
                        let failed_case = result
                            .as_ref()
                            .is_ok_and(|case| case.verdict != Verdict::Accepted);
                        if result.is_err() || failed_case {
                            failed = true;
                            stopped.store(true, Ordering::Release);
                        }
                        if sender.send((case_index, result)).is_err() {
                            failed = true;
                            break;
                        }
                        if failed {
                            break;
                        }
                    }
                    if failed {
                        continue;
                    }
                    if execution.status != 0 {
                        stopped.store(true, Ordering::Release);
                        let _ = sender.send((
                            index + count,
                            Err(anyhow::anyhow!(
                                "batch runner exited with status {}: {}",
                                execution.status,
                                execution.stderr
                            )),
                        ));
                        break;
                    }
                    if record_count != count {
                        stopped.store(true, Ordering::Release);
                        let _ = sender.send((
                            index + record_count,
                            Err(anyhow::anyhow!(
                                "batch runner returned {record_count} of {count} requested cases"
                            )),
                        ));
                        break;
                    }
                });
            }
            drop(sender);

            for (index, result) in receiver {
                match result {
                    Ok(result) => {
                        generated_completed += 1;
                        if let Some(log) = case_log.as_mut() {
                            if let Err(error) =
                                write_case_log(log, baseline_case_count as u64 + index, &result)
                            {
                                first_error.get_or_insert_with(|| error.to_string());
                                stopped.store(true, Ordering::Release);
                            }
                        }
                        if severity(result.verdict) > severity(worst_verdict) {
                            worst_verdict = result.verdict;
                        }
                        if result.verdict == Verdict::Accepted {
                            generated_passes += 1;
                            if generated_passes <= STORED_GENERATED_PASSES as u64 {
                                cases.push(result);
                            } else {
                                generated_omitted += 1;
                            }
                        } else {
                            generated_failures += 1;
                            stopped.store(true, Ordering::Release);
                            if generated_failures <= STORED_GENERATED_FAILURES as u64 {
                                cases.push(result);
                            } else {
                                generated_omitted += 1;
                            }
                        }
                        if generated_completed % 10_000 == 0 {
                            if let Some(log) = case_log.as_mut() {
                                if let Err(error) = log.flush() {
                                    first_error.get_or_insert_with(|| error.to_string());
                                    stopped.store(true, Ordering::Release);
                                }
                            }
                            eprintln!(
                                "ccx: tested {generated_completed}/{generated_cases} generated cases in {:.1}s",
                                stress_started.elapsed().as_secs_f64()
                            );
                        }
                    }
                    Err(error) => {
                        generated_completed += 1;
                        first_error.get_or_insert_with(|| error.to_string());
                        stopped.store(true, Ordering::Release);
                    }
                }
            }
        });

        if let Some(error) = first_error {
            if let Some(log) = case_log.as_mut() {
                log.flush()?;
            }
            return Err(Fixable::new(format!("generated test run stopped: {error}"))
                .fix("use the reported seed and size to reproduce this generator or runner failure")
                .into_error());
        }
        passed += generated_passes as usize;
        failed += generated_failures as usize;
        case_details_omitted = generated_omitted;
        cases[baseline_case_count..].sort_by_key(|case| case.sequence);
    }

    if let Some(log) = case_log.as_mut() {
        log.flush()?;
    }

    let overall = if failed == 0 {
        Verdict::Accepted
    } else {
        // The worst verdict wins, so a timeout is never reported as merely
        // "wrong answer".
        worst_verdict
    };

    let report = TestReport {
        problem: sol.problem.meta.id.clone(),
        sizes_tested: sizes,
        generated_size: generated_size.clone(),
        generated_cases: generated_completed,
        generated_cases_requested: generated_cases,
        complete: generated_cases == 0
            || (generated_attempted && generated_completed == generated_cases),
        case_details_omitted,
        case_log: case_log_path.map(|path| path.display().to_string()),
        cases,
        total_cases: (baseline_case_count as u64) + generated_completed,
        passed,
        failed,
        overall,
    };

    if let Some(log) = case_log.as_mut() {
        serde_json::to_writer(
            &mut *log,
            &CaseLogSummary {
                record_type: "summary",
                complete: report.complete,
                total_cases: report.total_cases,
                passed: report.passed,
                failed: report.failed,
            },
        )?;
        log.write_all(b"\n")?;
        log.flush()?;
    }

    cmd::emit(
        format,
        || {
            println!("{}: {passed} passed, {failed} failed", report.problem);
            if report.generated_cases_requested > 0 {
                println!(
                    "  tested {} of {} requested generated cases at size {} from seed {}{}",
                    report.generated_cases,
                    report.generated_cases_requested,
                    report.generated_size.as_deref().unwrap_or("unknown"),
                    first_seed,
                    case_log_path
                        .map(|path| format!("; full case log: {}", path.display()))
                        .unwrap_or_default()
                );
                if report.case_details_omitted > 0 && case_log_path.is_none() {
                    println!(
                        "  {} generated case details omitted; pass --case-log <path> to retain every result",
                        report.case_details_omitted
                    );
                }
            }
            for c in &report.cases {
                let mark = if c.verdict == Verdict::Accepted {
                    "ok  "
                } else {
                    "FAIL"
                };
                let mut line = format!(
                    "  {mark} {:>8}  seed {:>12} ({})  {}  wall={:.3}ms  peak_rss={}B",
                    c.size,
                    c.seed,
                    c.seed_set,
                    c.verdict,
                    c.wall_ns as f64 / 1_000_000.0,
                    c.peak_rss_bytes
                );
                if let Some(resources) = &c.resources {
                    if let Some(cpu_ns) = resources.user_cpu_ns {
                        line.push_str(&format!("  user_cpu={:.3}ms", cpu_ns as f64 / 1_000_000.0));
                    }
                    if let Some(cpu_ns) = resources.system_cpu_ns {
                        line.push_str(&format!(
                            "  system_cpu={:.3}ms",
                            cpu_ns as f64 / 1_000_000.0
                        ));
                    }
                }
                if let Some(d) = &c.first_difference {
                    line.push_str(&format!("\n        {d}"));
                }
                println!("{line}");
            }
        },
        &report,
    )?;

    if report.overall != Verdict::Accepted {
        return Err(Fixable::new(format!("{} test(s) failed", report.failed))
            .fix("use `--case-log <path>` to retain every case result in large generated runs")
            .fix("the first difference in each failing case is shown above")
            .into_error());
    }
    Ok(())
}

fn write_case_log(log: &mut BufWriter<File>, index: u64, case: &CaseResult) -> anyhow::Result<()> {
    serde_json::to_writer(
        &mut *log,
        &CaseLogEntry {
            record_type: "case",
            case_index: index,
            case,
        },
    )?;
    log.write_all(b"\n")?;
    Ok(())
}

fn parse_batch_records(output: &str) -> anyhow::Result<Vec<BatchCaseRecord>> {
    output
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .map_err(|error| anyhow::anyhow!("batch runner emitted invalid case JSON: {error}"))
        })
        .collect()
}

fn case_from_batch(
    record: BatchCaseRecord,
    size: &str,
    seed: u64,
    sequence: u64,
    policy: &ccx_core::sandbox::SandboxPolicy,
    checker_used: bool,
) -> anyhow::Result<CaseResult> {
    if let Some(stage) = record.stage_error {
        return Err(anyhow::anyhow!("the {stage} failed for seed {seed}"));
    }
    if record.seed != seed {
        return Err(anyhow::anyhow!(
            "batch runner returned seed {}, expected {seed}",
            record.seed
        ));
    }
    for (stage, process) in [
        ("generator", record.generator.as_ref()),
        ("reference", record.reference.as_ref()),
    ] {
        let process = process.ok_or_else(|| {
            anyhow::anyhow!("batch runner omitted the {stage} result for seed {seed}")
        })?;
        if process.exit_status != 0
            || process.signal != 0
            || process.timed_out
            || process.output_truncated
        {
            return Err(anyhow::anyhow!("the {stage} failed for seed {seed}"));
        }
    }
    let solution = record.solution.ok_or_else(|| {
        anyhow::anyhow!("batch runner omitted the solution result for seed {seed}")
    })?;
    let outcome = solution.outcome();
    let verdict = outcome.verdict(policy.memory_bytes);
    if checker_used && verdict == Verdict::Accepted && record.checker.is_none() {
        return Err(anyhow::anyhow!(
            "batch runner omitted the checker result for seed {seed}"
        ));
    }
    let accepted = if checker_used {
        record.checker_accepted
    } else {
        record.outputs_equal
    };
    let first_difference = if verdict != Verdict::Accepted || accepted {
        None
    } else if checker_used {
        Some("checker rejected the candidate output".into())
    } else {
        Some(format!(
            "output differs at byte {}; expected {} bytes, got {}",
            record
                .difference_offset
                .unwrap_or(record.expected_bytes.min(record.actual_bytes)),
            record.expected_bytes,
            record.actual_bytes
        ))
    };
    let final_verdict = if first_difference.is_some() {
        Verdict::WrongAnswer
    } else {
        verdict
    };
    Ok(CaseResult {
        sequence: Some(sequence),
        size: size.to_string(),
        seed: record.seed,
        seed_set: "generated".into(),
        verdict: final_verdict,
        exit_status: outcome.exit_status,
        signal: outcome.signal,
        wall_ns: outcome.wall_ns,
        peak_rss_bytes: outcome.peak_rss_bytes,
        resources: outcome.resources,
        first_difference,
    })
}

fn execute_case(
    pipeline: &Pipeline<'_>,
    generator: &crate::pipeline::Program,
    reference: &crate::pipeline::Program,
    program: &crate::pipeline::Program,
    checker: Option<&crate::pipeline::Program>,
    policy: &ccx_core::sandbox::SandboxPolicy,
    size: &str,
    seed: u64,
    seed_set: &str,
    sequence: Option<u64>,
) -> anyhow::Result<CaseResult> {
    let input = pipeline.generate_with(generator, size, seed)?;
    let expected = pipeline.reference_output_with(reference, &input)?;
    let outcome = pipeline.run_program(program, &input, policy, false)?;
    let verdict = outcome.verdict(policy.memory_bytes);

    let diff = if verdict == Verdict::Accepted {
        let accepted = match checker {
            Some(checker) => pipeline.check_output(
                checker,
                &input,
                &expected,
                outcome.stdout.as_bytes(),
                policy,
            )?,
            None => expected.as_slice() == outcome.stdout.as_bytes(),
        };
        if accepted {
            None
        } else if checker.is_some() {
            Some("checker rejected the output".to_string())
        } else {
            first_difference(&expected, outcome.stdout.as_bytes())
        }
    } else {
        None
    };

    let final_verdict = match (&diff, verdict) {
        (Some(_), Verdict::Accepted) => Verdict::WrongAnswer,
        _ => verdict,
    };

    Ok(CaseResult {
        sequence,
        size: size.to_string(),
        seed,
        seed_set: seed_set.to_string(),
        verdict: final_verdict,
        exit_status: outcome.exit_status,
        signal: outcome.signal,
        wall_ns: outcome.wall_ns,
        peak_rss_bytes: outcome.peak_rss_bytes,
        resources: outcome.resources,
        first_difference: diff,
    })
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

#[cfg(test)]
mod tests {
    use super::{
        case_from_batch, parse_batch_records, truncate, validate_stress_request, CaseResult,
        MAX_CASES, MAX_JOBS,
    };
    use ccx_core::capsule::Verdict;
    use ccx_core::engine::ResourceUsage;

    fn batch_record(equal: bool) -> String {
        let process = serde_json::json!({
            "exit_status": 0,
            "signal": 0,
            "timed_out": false,
            "output_truncated": false,
            "peak_rss_bytes": 4096,
            "wall_ns": 1234,
            "stdout_bytes": 2,
            "stderr_bytes": 0,
            "user_cpu_ns": 100,
            "system_cpu_ns": 20,
            "minor_page_faults": 1,
            "major_page_faults": 0,
            "voluntary_context_switches": 2,
            "involuntary_context_switches": 0,
            "block_input_ops": 0,
            "block_output_ops": 0
        });
        serde_json::json!({
            "seed": 42,
            "stage_error": null,
            "generator": process,
            "reference": process,
            "solution": process,
            "checker": null,
            "checker_accepted": equal,
            "expected_bytes": 2,
            "actual_bytes": if equal { 2 } else { 3 },
            "outputs_equal": equal,
            "difference_offset": if equal { serde_json::Value::Null } else { serde_json::json!(1) }
        })
        .to_string()
    }

    #[test]
    fn accepts_million_case_seed_ranges() {
        assert!(validate_stress_request(MAX_CASES, 1, MAX_JOBS).is_ok());
    }

    #[test]
    fn rejects_seed_overflow() {
        assert!(validate_stress_request(2, u64::MAX, 1).is_err());
    }

    #[test]
    fn rejects_excessive_worker_count() {
        assert!(validate_stress_request(1, 1, MAX_JOBS + 1).is_err());
    }

    #[test]
    fn truncation_keeps_utf8_boundaries() {
        let value = format!("{}x", "é".repeat(80));
        assert_eq!(truncate(&value), format!("{}…", "é".repeat(60)));
    }

    #[test]
    fn case_report_serializes_process_resource_counters() {
        let case = CaseResult {
            sequence: Some(3),
            size: "naive".into(),
            seed: 42,
            seed_set: "generated".into(),
            verdict: Verdict::Accepted,
            exit_status: 0,
            signal: 0,
            wall_ns: 1_000,
            peak_rss_bytes: 2_000,
            resources: Some(ResourceUsage {
                user_cpu_ns: Some(100),
                system_cpu_ns: Some(200),
                minor_page_faults: Some(3),
                major_page_faults: Some(4),
                voluntary_context_switches: Some(5),
                involuntary_context_switches: Some(6),
                block_input_ops: Some(7),
                block_output_ops: Some(8),
            }),
            first_difference: None,
        };

        let value = serde_json::to_value(case).unwrap();
        assert_eq!(value["resources"]["user_cpu_ns"], 100);
        assert_eq!(value["resources"]["major_page_faults"], 4);
        assert_eq!(value["resources"]["block_output_ops"], 8);
    }

    #[test]
    fn batch_records_preserve_process_usage_and_detect_wrong_answers() {
        let output = format!("{}\n", batch_record(false));
        let mut records = parse_batch_records(&output).unwrap();
        let case = case_from_batch(
            records.remove(0),
            "naive",
            42,
            0,
            &ccx_core::sandbox::SandboxPolicy::default(),
            false,
        )
        .unwrap();
        assert_eq!(case.verdict, Verdict::WrongAnswer);
        assert_eq!(
            case.first_difference.as_deref(),
            Some("output differs at byte 1; expected 2 bytes, got 3")
        );
        assert_eq!(case.resources.unwrap().user_cpu_ns, Some(100));
    }

    #[test]
    fn batch_records_accept_matching_output() {
        let records = parse_batch_records(&batch_record(true)).unwrap();
        let case = case_from_batch(
            records.into_iter().next().unwrap(),
            "naive",
            42,
            0,
            &ccx_core::sandbox::SandboxPolicy::default(),
            false,
        )
        .unwrap();
        assert_eq!(case.verdict, Verdict::Accepted);
        assert!(case.first_difference.is_none());
    }
}
