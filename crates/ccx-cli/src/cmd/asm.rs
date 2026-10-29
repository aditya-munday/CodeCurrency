//! `ccx asm` — emit assembly with source mapping and compiler remarks.
//!
//! This is the explanation layer's raw material. The counter tells you *what*
//! changed; the assembly and the compiler's own remarks tell you *why*. Both
//! come from the same pinned toolchain as the measurement, so they describe the
//! same build.

use crate::cmd;
use crate::context::Solution;
use crate::pipeline::Pipeline;
use crate::Format;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct AsmReport {
    pub toolchain: String,
    pub assembly_path: String,
    /// Assembly with `#` source-line markers, first 8 KiB.
    pub assembly: String,
    /// Compiler optimisation remarks, first 8 KiB.
    pub remarks: String,
    pub remarks_status: String,
    pub remarks_method: Option<String>,
    pub remarks_explanation: Option<String>,
    pub assembly_files: Vec<AssemblyFileReport>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AssemblyFileReport {
    pub source: String,
    pub path: String,
    pub assembly: String,
}

pub fn run(format: Format, dir: &Path) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let pipeline = Pipeline::new(&sol)?;
    let tc = sol.toolchain()?;
    sol.ensure_dirs()?;

    // Assembly: `-S` plus source interleaving, so every instruction can be tied
    // back to a line the author wrote.
    let outputs = pipeline.build_assembly()?;
    let assembly_files: Vec<AssemblyFileReport> = outputs
        .iter()
        .map(|(source, path)| AssemblyFileReport {
            source: source.clone(),
            path: path.display().to_string(),
            assembly: std::fs::read_to_string(path)
                .unwrap_or_default()
                .chars()
                .take(8192)
                .collect(),
        })
        .collect();
    let first_output = assembly_files
        .first()
        .ok_or_else(|| anyhow::anyhow!("assembly generation returned no files"))?;

    // Remarks: GCC's `-fopt-info` narrates what it did to each loop. This is the
    // compiler explaining itself, which is exactly what a hint should cite.
    let remarks = pipeline.optimization_remarks()?;

    let report = AsmReport {
        toolchain: format!("{} {}", tc.name.profile_id(), tc.version),
        assembly_path: first_output.path.clone(),
        assembly: first_output.assembly.clone(),
        remarks: remarks.text.chars().take(8192).collect(),
        remarks_status: remarks.status,
        remarks_method: remarks.method,
        remarks_explanation: remarks.explanation,
        assembly_files,
    };

    cmd::emit(
        format,
        || {
            println!("assembly ({})", report.toolchain);
            println!("  full assembly: {}", report.assembly_path);
            if report.assembly_files.len() > 1 {
                for file in &report.assembly_files {
                    println!("  {}: {}", file.source, file.path);
                }
            }
            for line in report.assembly.lines().take(80) {
                println!("  {line}");
            }
            if report.remarks_status == "unavailable" {
                println!();
                println!(
                    "compiler remarks unavailable: {}",
                    report
                        .remarks_explanation
                        .as_deref()
                        .unwrap_or("no reason provided")
                );
            } else {
                println!();
                println!(
                    "compiler remarks ({}):",
                    report
                        .remarks_method
                        .as_deref()
                        .unwrap_or("unspecified method")
                );
                if let Some(explanation) = &report.remarks_explanation {
                    println!("  {explanation}");
                }
                for line in report.remarks.lines().take(30) {
                    println!("  {line}");
                }
            }
        },
        &report,
    )?;

    Ok(())
}
