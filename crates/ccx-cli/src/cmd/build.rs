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
#[allow(dead_code)]
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
    if sol.language == Language::Rust {
        let opt = sol.opt_level.trim_start_matches("-O");
        return Ok((
            vec!["--edition=2021".to_string(), format!("-Copt-level={opt}")],
            Vec::new(),
        ));
    }
    if sol.language == Language::D {
        let flags = vec![sol.opt_level.clone()];
        let check = check_flags(&flags);
        if !check.ok() {
            return Err(Fixable::new("the toolchain refused a default build flag")
                .fix("this is an engine bug: the default flag set must always be accepted")
                .fix(format!("refused: {:#?}", check.rejected))
                .into_error());
        }
        return Ok((check.accepted, check.rejected));
    }
    if sol.language == Language::Zig {
        let mode = match sol.opt_level.as_str() {
            "-O0" => "Debug",
            "-Os" | "-Oz" => "ReleaseSmall",
            _ => "ReleaseFast",
        };
        return Ok((
            vec![
                "build-exe".to_string(),
                format!("-O{mode}"),
                "-lc".to_string(),
            ],
            Vec::new(),
        ));
    }
    if sol.language == Language::Odin {
        let optimization = if sol.opt_level == "-O0" {
            "-debug"
        } else {
            "-o:speed"
        };
        return Ok((
            vec![
                "build".to_string(),
                "-file".to_string(),
                optimization.to_string(),
                "-out:/work/scratch/sol".to_string(),
            ],
            Vec::new(),
        ));
    }
    if sol.language == Language::Go {
        return Ok((
            vec![
                "build".to_string(),
                "-trimpath".to_string(),
                "-buildvcs=false".to_string(),
                "-ldflags=-buildid=".to_string(),
            ],
            Vec::new(),
        ));
    }
    if sol.language == Language::AsmX86_64 {
        return Ok((
            vec![
                "-ffile-prefix-map=/in=.".to_string(),
                "-fno-ident".to_string(),
                "-Wl,--build-id=none".to_string(),
            ],
            Vec::new(),
        ));
    }
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

/// Compile the solution. The binary lands in `.ccx/out/sol-<sha256>`.
///
/// This delegates to the pipeline's shared builder so `build`, `test`,
/// `measure`, and `replay` all compile the same way: the compiler runs as the
/// runner's child, and the binary is framed back out of the sandbox.
pub fn compile(sol: &Solution) -> anyhow::Result<BuildReport> {
    let pipeline = crate::pipeline::Pipeline::new(sol)?;
    let tc = sol.toolchain()?;
    let program = pipeline.build_solution()?;

    let temporary_binary = program.host_dir.join("sol");
    let bytes = std::fs::read(&temporary_binary)?;
    let binary_sha256 = ccx_core::capsule::sha256_hex(&bytes);
    let host_binary = sol.out_dir().join(format!("sol-{binary_sha256}"));
    std::fs::create_dir_all(sol.out_dir())?;
    if host_binary.exists() {
        if std::fs::read(&host_binary)? != bytes {
            anyhow::bail!("content-addressed build artifact did not match its SHA-256 path");
        }
    } else {
        match std::fs::rename(&temporary_binary, &host_binary) {
            Ok(()) => {}
            Err(error) if host_binary.exists() => {
                if std::fs::read(&host_binary)? != bytes {
                    return Err(error.into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    let (_flags, rejected) = build_flags(sol)?;

    // The argv is reconstructed for the report: it is what a person needs to
    // reproduce the build by hand.
    let compiler_argv = pipeline.build_argv();

    Ok(BuildReport {
        toolchain: tc.name.profile_id().into(),
        version: tc.version,
        image_digest: tc.image_digest,
        opt_level: if sol.language == Language::AsmX86_64 {
            "none".to_string()
        } else {
            sol.opt_level.clone()
        },
        compiler_argv,
        rejected_flags: rejected,
        binary: host_binary.display().to_string(),
        binary_bytes: bytes.len() as u64,
        binary_sha256,
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
