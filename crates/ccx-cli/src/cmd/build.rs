//! `ccx build` — compile a solution inside the pinned toolchain image.
//!
//! The build runs in the same sandbox as the run, for the same reason: a
//! compiler is an attacker-controlled program when its input is a hostile
//! source file. The source is mounted read-only, scratch is tmpfs, and the
//! binary is copied out afterwards with `docker cp`. No writable host mount
//! exists at any point.
//!
//! Flags come from `toolchain::check_flags`, so a solution cannot smuggle in
//! `-fplugin`, an `@response-file`, or an absolute `-I` that reaches outside
//! the sandbox. Rejected flags are reported, never silently dropped.

use crate::cmd::{self, Fixable};
use crate::context::Solution;
use crate::Format;
use ccx_core::capsule::Language;
use ccx_core::toolchain::{check_flags, Rejected};
use serde::Serialize;
use std::path::Path;

/// Guest path of the compiled binary inside scratch.
pub const GUEST_BINARY: &str = "/work/scratch/sol";

/// The result of a successful build.
#[derive(Debug, Clone, Serialize)]
pub struct BuildReport {
    pub toolchain: String,
    pub version: String,
    pub image_digest: String,
    pub opt_level: String,
    /// Compiler argv as run inside the container.
    pub compiler_argv: Vec<String>,
    /// Flags the solution asked for that were refused, with reasons.
    pub rejected_flags: Vec<Rejected>,
    /// Path to the built binary on the host.
    pub binary: String,
    pub binary_bytes: u64,
    /// SHA-256 of the built binary, so two builds can be compared.
    pub binary_sha256: String,
    pub warnings: Vec<String>,
}

/// The compiler flags for a solution.
///
/// Phase 1 keeps this surface fixed per language: the point is to compare
/// algorithms, not compiler incantations. `-O2` plus the standard version is
/// the whole set, and both must pass the flag checker.
fn build_flags(sol: &Solution) -> anyhow::Result<(Vec<String>, Vec<Rejected>)> {
    let std_flag = match sol.language {
        Language::C => "-std=c17",
        _ => "-std=c++20",
    };
    let flags = vec![sol.opt_level.clone(), std_flag.to_string()];
    let check = check_flags(&flags);
    if !check.ok() {
        return Err(Fixable::new("the toolchain refused a default build flag")
            .fix("this is an engine bug: the default flag set must always be accepted")
            .fix(format!("refused: {:#?}", check.rejected))
            .into_error());
    }
    Ok((check.accepted, check.rejected))
}

/// Compile the solution. The binary lands in `.ccx/out/sol`.
///
/// This delegates to the pipeline's shared builder so `build`, `test`,
/// `measure`, and `replay` all compile the same way: the compiler runs as the
/// runner's child, and the binary is framed back out of the sandbox.
pub fn compile(sol: &Solution) -> anyhow::Result<BuildReport> {
    let pipeline = crate::pipeline::Pipeline::new(sol)?;
    let tc = sol.toolchain()?;
    let program = pipeline.build_solution()?;

    let host_binary = program.host_dir.join("sol");
    let bytes = std::fs::read(&host_binary)?;
    let (flags, rejected) = build_flags(sol)?;

    // The argv is reconstructed for the report: it is what a person needs to
    // reproduce the build by hand.
    let driver = ccx_core::toolchain::driver_for(sol.toolchain_name, sol.language);
    let mut compiler_argv = vec![driver.to_string()];
    compiler_argv.extend(flags);
    compiler_argv.push(format!("-o {GUEST_BINARY}"));

    Ok(BuildReport {
        toolchain: format!("{:?}", tc.name).to_lowercase(),
        version: tc.version,
        image_digest: tc.image_digest,
        opt_level: sol.opt_level.clone(),
        compiler_argv,
        rejected_flags: rejected,
        binary: host_binary.display().to_string(),
        binary_bytes: bytes.len() as u64,
        binary_sha256: ccx_core::capsule::sha256_hex(&bytes),
        warnings: Vec::new(),
    })
}

pub fn run(format: Format, dir: &Path) -> anyhow::Result<()> {
    let sol = Solution::load(dir)?;
    let report = compile(&sol)?;

    cmd::emit(
        format,
        || {
            let short = &report.image_digest[..19.min(report.image_digest.len())];
            println!("built {} {} ({short})", report.toolchain, report.version);
            println!("  opt       {}", report.opt_level);
            println!("  binary    {} bytes", report.binary_bytes);
            println!("  sha256    {}", &report.binary_sha256[..16]);
            if !report.rejected_flags.is_empty() {
                println!("  refused   {} flag(s)", report.rejected_flags.len());
                for r in &report.rejected_flags {
                    println!("    {} — {}", r.flag, r.reason);
                }
            }
            if !report.warnings.is_empty() {
                println!("  warnings  {}", report.warnings.len());
                for w in report.warnings.iter().take(5) {
                    println!("    {w}");
                }
            }
        },
        &report,
    )
}
