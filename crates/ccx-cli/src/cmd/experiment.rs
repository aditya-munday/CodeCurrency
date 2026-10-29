use crate::cmd::{self, Fixable};
use crate::Format;
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

#[derive(Debug, Clone, Serialize)]
pub struct ExperimentReport {
    pub status: String,
    pub problem: String,
    pub correctness: Value,
    pub measurement: Option<Value>,
    pub assembly: Option<Value>,
    pub error: Option<String>,
}

fn invoke(arguments: &[String]) -> anyhow::Result<(ExitStatus, String)> {
    let executable = std::env::current_exe()?;
    let output = Command::new(executable)
        .arg("--format")
        .arg("json")
        .args(arguments)
        .stderr(Stdio::inherit())
        .output()?;
    Ok((
        output.status,
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

pub fn run(
    format: Format,
    dir: &Path,
    repeat: u32,
    out: Option<&Path>,
    size: &str,
    seed: u64,
    cases: u64,
    jobs: usize,
    case_log: Option<&Path>,
) -> anyhow::Result<()> {
    let mut test_arguments = vec![
        "test".to_string(),
        "--dir".to_string(),
        dir.display().to_string(),
        "--hidden".to_string(),
        "--cases".to_string(),
        cases.to_string(),
        "--seed".to_string(),
        seed.to_string(),
        "--size".to_string(),
        size.to_string(),
        "--jobs".to_string(),
        jobs.to_string(),
    ];
    if let Some(path) = case_log {
        test_arguments.push("--case-log".to_string());
        test_arguments.push(path.display().to_string());
    }

    let (test_status, test_stdout) = invoke(&test_arguments)?;
    let correctness: Value = serde_json::from_str(&test_stdout).map_err(|error| {
        Fixable::new(format!(
            "the correctness stage did not return a test report: {error}"
        ))
        .fix("run `ccx test --hidden` directly to inspect the test-stage error")
        .into_error()
    })?;
    let problem = correctness["problem"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();

    if !test_status.success()
        || correctness["overall"] != "accepted"
        || correctness["complete"] != true
    {
        let report = ExperimentReport {
            status: "correctness_failed".to_string(),
            problem,
            correctness,
            measurement: None,
            assembly: None,
            error: Some(
                "the public, hidden, or generated correctness suite did not pass completely"
                    .to_string(),
            ),
        };
        emit_report(format, &report)?;
        return Err(
            Fixable::new("measurement was skipped because correctness did not pass")
                .fix("inspect the correctness report and fix the first failing seed")
                .into_error(),
        );
    }

    let mut measure_arguments = vec![
        "measure".to_string(),
        "--dir".to_string(),
        dir.display().to_string(),
        "--repeat".to_string(),
        repeat.to_string(),
        "--size".to_string(),
        size.to_string(),
        "--seed".to_string(),
        seed.to_string(),
    ];
    if let Some(path) = out {
        measure_arguments.push("--out".to_string());
        measure_arguments.push(path.display().to_string());
    }

    let (measure_status, measure_stdout) = invoke(&measure_arguments)?;
    let measurement = serde_json::from_str(&measure_stdout).ok();
    if !measure_status.success() || measurement.is_none() {
        let report = ExperimentReport {
            status: "measurement_failed".to_string(),
            problem,
            correctness,
            measurement,
            assembly: None,
            error: Some("the measurement command did not return a valid report".to_string()),
        };
        emit_report(format, &report)?;
        return Err(Fixable::new("measurement failed after correctness passed")
            .fix("inspect the solution build and measurement configuration")
            .into_error());
    }

    let assembly_arguments = vec![
        "asm".to_string(),
        "--dir".to_string(),
        dir.display().to_string(),
    ];
    let (assembly_status, assembly_stdout) = invoke(&assembly_arguments)?;
    let assembly = serde_json::from_str(&assembly_stdout).ok();
    let assembly_failed = !assembly_status.success() || assembly.is_none();
    let report = ExperimentReport {
        status: if assembly_failed {
            "explanation_failed"
        } else {
            "accepted"
        }
        .to_string(),
        problem,
        correctness,
        measurement,
        assembly,
        error: assembly_failed
            .then(|| "the assembly explanation command did not return a valid report".to_string()),
    };
    emit_report(format, &report)?;
    if report.status != "accepted" {
        return Err(
            Fixable::new("assembly explanation failed after measurement passed")
                .fix("run `ccx asm` directly to inspect the toolchain explanation error")
                .into_error(),
        );
    }
    Ok(())
}

fn emit_report(format: Format, report: &ExperimentReport) -> anyhow::Result<()> {
    cmd::emit(
        format,
        || {
            println!("experiment {}: {}", report.problem, report.status);
            let passed = report.correctness["passed"].as_u64().unwrap_or(0);
            let total = report.correctness["total_cases"].as_u64().unwrap_or(0);
            println!("  correctness  {passed}/{total} cases passed");
            if let Some(size) = report.correctness["generated_size"].as_str() {
                println!(
                    "  stress       {}/{} cases at size {}",
                    report.correctness["generated_cases"].as_u64().unwrap_or(0),
                    report.correctness["generated_cases_requested"]
                        .as_u64()
                        .unwrap_or(0),
                    size
                );
            }
            if let Some(measurement) = &report.measurement {
                println!(
                    "  target       {} | {} {} | {} | {} CPUs",
                    measurement["execution_target"]
                        .as_str()
                        .unwrap_or("unknown"),
                    measurement["host"]["cpu_model"]
                        .as_str()
                        .unwrap_or("unknown"),
                    measurement["host"]["os"].as_str().unwrap_or("unknown"),
                    measurement["host"]["kernel"].as_str().unwrap_or("unknown"),
                    measurement["host"]["nproc"].as_u64().unwrap_or(0)
                );
                println!(
                    "  toolchain    {} {} | environment {}",
                    measurement["toolchain"]["name"]
                        .as_str()
                        .unwrap_or("unknown"),
                    measurement["toolchain"]["version"]
                        .as_str()
                        .unwrap_or("unknown"),
                    measurement["toolchain"]["environment_fingerprint"]
                        .as_str()
                        .unwrap_or("legacy image digest only")
                );
                println!(
                    "  capsule      {}",
                    measurement["capsule_id"].as_str().unwrap_or("unknown")
                );
                println!(
                    "  replay       ccx replay {}",
                    measurement["capsule_path"].as_str().unwrap_or("<capsule>")
                );
                println!(
                    "  ranked on    {}",
                    measurement["ranked_metric"].as_str().unwrap_or("unknown")
                );
                if measurement["rankability"]["eligible"] == true {
                    println!(
                        "  rankability  eligible | n={} minimum={} runs, spread {:.4}% <= {:.4}% same-host tolerance",
                        measurement["runs"].as_u64().unwrap_or_default(),
                        measurement["rankability"]["minimum_runs"]
                            .as_u64()
                            .unwrap_or_default(),
                        measurement["rankability"]["observed_spread_pct"]
                            .as_f64()
                            .unwrap_or_default(),
                        measurement["rankability"]["tolerance_pct"]
                            .as_f64()
                            .unwrap_or_default()
                    );
                } else {
                    println!(
                        "  rankability  ineligible | n={} minimum={} | {}",
                        measurement["runs"].as_u64().unwrap_or_default(),
                        measurement["rankability"]["minimum_runs"]
                            .as_u64()
                            .unwrap_or_default(),
                        measurement["rankability"]["reason"]
                            .as_str()
                            .unwrap_or("repeatability has not been established")
                    );
                }
                if let Some(metrics) = measurement["metrics"].as_array() {
                    for metric in metrics {
                        let value = metric["value"].as_f64().unwrap_or_default();
                        let unit = metric["unit"].as_str().unwrap_or("");
                        let formatted = if matches!(unit, "ratio" | "instructions/cycle") {
                            format!("{value:.6}")
                        } else if unit == "hertz" || value.abs() >= 1e6 {
                            format!("{value:.3e}")
                        } else {
                            format!("{value:.0}")
                        };
                        println!(
                            "  {:<28} {:>14} {:<14} {} ({})",
                            metric["name"].as_str().unwrap_or("unknown"),
                            formatted,
                            unit,
                            metric["kind"].as_str().unwrap_or("unknown"),
                            metric["method"].as_str().unwrap_or("unknown")
                        );
                        if let Some(spread) = metric["repeatability"].as_object() {
                            let repeatability_value = |name: &str| {
                                spread
                                    .get(name)
                                    .and_then(Value::as_f64)
                                    .map(|value| {
                                        if matches!(unit, "ratio" | "instructions/cycle") {
                                            format!("{value:.6}")
                                        } else if unit == "hertz" || value.abs() >= 1e6 {
                                            format!("{value:.3e}")
                                        } else {
                                            format!("{value:.0}")
                                        }
                                    })
                                    .unwrap_or_else(|| "n/a".to_string())
                            };
                            println!(
                                "    n={} p95={} p99={} range=[{}, {}] spread={:.4}%",
                                measurement["runs"].as_u64().unwrap_or(0),
                                repeatability_value("p95"),
                                repeatability_value("p99"),
                                repeatability_value("min"),
                                repeatability_value("max"),
                                spread
                                    .get("spread_pct")
                                    .and_then(Value::as_f64)
                                    .unwrap_or_default()
                            );
                        }
                        if let Some(half_width) = metric["uncertainty"].as_f64() {
                            let uncertainty_kind =
                                metric["uncertainty_kind"].as_str().unwrap_or("uncertainty");
                            let label = if uncertainty_kind == "ci95" {
                                "95% CI half-width"
                            } else {
                                "uncertainty"
                            };
                            println!(
                                "    {}={} {}",
                                label,
                                if matches!(unit, "ratio" | "instructions/cycle") {
                                    format!("{half_width:.6}")
                                } else if unit == "hertz" || half_width.abs() >= 1e6 {
                                    format!("{half_width:.3e}")
                                } else {
                                    format!("{half_width:.0}")
                                },
                                unit
                            );
                        }
                    }
                }
                if let Some(hints) = measurement["hints"].as_array() {
                    if !hints.is_empty() {
                        println!("\n  explanation");
                        for hint in hints {
                            if let Some(message) = hint["message"].as_str() {
                                println!("    • {message}");
                            }
                        }
                    }
                }
            }
            if let Some(assembly) = &report.assembly {
                let files = assembly["assembly_files"].as_array();
                println!(
                    "\n  assembly     {}",
                    assembly["toolchain"]
                        .as_str()
                        .unwrap_or("unknown toolchain")
                );
                if let Some(files) = files {
                    for file in files {
                        println!(
                            "    {}: {}",
                            file["source"].as_str().unwrap_or("source"),
                            file["path"].as_str().unwrap_or("assembly artifact")
                        );
                    }
                }
                println!(
                    "  compiler remarks  {} ({})",
                    assembly["remarks_status"].as_str().unwrap_or("unknown"),
                    assembly["remarks_method"].as_str().unwrap_or("no method")
                );
                if let Some(explanation) = assembly["remarks_explanation"].as_str() {
                    println!("    {explanation}");
                }
                if let Some(remarks) = assembly["remarks"].as_str() {
                    for line in remarks.lines().take(12) {
                        println!("    {line}");
                    }
                }
            }
            if let Some(error) = &report.error {
                println!("  error        {error}");
            }
        },
        report,
    )
}
