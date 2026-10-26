//! `ccx asm` — emit assembly with source mapping and compiler remarks.
//!
//! This is the explanation layer's raw material. The counter tells you *what*
//! changed; the assembly and the compiler's own remarks tell you *why*. Both
//! come from the same pinned toolchain as the measurement, so they describe the
//! same build.

use crate::cmd::{self, Fixable};
use crate::context::Solution;
use crate::Format;
use ccx_core::sandbox::SandboxPolicy;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct AsmReport {
    pub toolchain: String,
    /// Assembly with `#` source-line markers, first 8 KiB.
    pub assembly: String,
    /// Compiler optimisation remarks (`-fopt-info`), first 8 KiB.
    pub remarks: String,
}

pub fn run(format: Format, dir: &Path) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let docker = crate::context::require_docker()?;
    let tc = sol.toolchain()?;
    sol.ensure_dirs()?;

    let image = format!("{}@{}", tc.image, tc.image_digest);
    let src_dir = sol.root.to_string_lossy().to_string();

    // Assembly: `-S` plus source interleaving, so every instruction can be tied
    // back to a line the author wrote.
    let mut argv = vec![tc.compiler.clone()];
    argv.extend([
        sol.opt_level.clone(),
        "-fverbose-asm".to_string(),
        "-masm=intel".to_string(),
        "-S".to_string(),
        "-o".to_string(),
        "/work/scratch/out.s".to_string(),
    ]);
    for name in sol.sources.keys() {
        argv.push(format!("/in/{name}"));
    }

    let mut policy = SandboxPolicy::default();
    policy.timeout_ms = 120_000;
    policy.cpuset = None;
    let container = format!("ccx-asm-{}", std::process::id());
    let docker_argv = policy.argv_named_mounted(
        &image,
        &argv,
        &container,
        &[(src_dir.clone(), "/in".to_string())],
    );

    let host_asm = sol.out_dir().join("out.s");
    let (status, stdout, stderr) =
        docker.run_and_extract(docker_argv, &container, "/work/scratch/out.s", &host_asm)?;
    if status != 0 {
        let diag = if stderr.trim().is_empty() { &stdout } else { &stderr };
        return Err(Fixable::new(format!("emitting assembly failed (exit {status})\n\n{diag}"))
            .fix("run `ccx build` first: the same errors will be clearer")
            .into_error());
    }
    let assembly = std::fs::read_to_string(&host_asm).unwrap_or_default();

    // Remarks: GCC's `-fopt-info` narrates what it did to each loop. This is the
    // compiler explaining itself, which is exactly what a hint should cite.
    let mut rem_argv = vec![tc.compiler.clone()];
    rem_argv.extend([
        sol.opt_level.clone(),
        "-fopt-info-all=remark.txt".to_string(),
        "-c".to_string(),
        "-o".to_string(),
        "/dev/null".to_string(),
    ]);
    for name in sol.sources.keys() {
        rem_argv.push(format!("/in/{name}"));
    }

    let container = format!("ccx-rem-{}", std::process::id());
    let rem_docker = policy.argv_named_mounted(
        &image,
        &rem_argv,
        &container,
        &[(src_dir.clone(), "/in".to_string())],
    );

    let host_rem = sol.out_dir().join("remarks.txt");
    let _ = docker.run_and_extract(
        rem_docker,
        &container,
        &format!("/work/scratch/remark.txt"),
        &host_rem,
    );
    let remarks = std::fs::read_to_string(&host_rem).unwrap_or_default();

    let report = AsmReport {
        toolchain: format!("{:?} {}", tc.name, tc.version).to_lowercase(),
        assembly: assembly.chars().take(8192).collect(),
        remarks: remarks.chars().take(8192).collect(),
    };

    cmd::emit(
        format,
        || {
            println!("assembly ({})", report.toolchain);
            for line in report.assembly.lines().take(80) {
                println!("  {line}");
            }
            if !report.remarks.is_empty() {
                println!();
                println!("compiler remarks:");
                for line in report.remarks.lines().take(30) {
                    println!("  {line}");
                }
            }
        },
        &report,
    )?;

    Ok(())
}
