//! `ccx doctor` — report what the machine can and cannot do.
//!
//! Every result depends on the environment, so the environment is checked
//! first and stated plainly. Anything missing is reported with the fix, never
//! silently worked around.

use crate::cmd::{self, Fixable};
use crate::Format;
use ccx_core::capsule::ToolchainName;
use ccx_core::engine::{parse_framed_report, Docker};
use ccx_core::measure::read_perf_event_paranoid;
use ccx_core::toolchain;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub fix: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<Check>,
    pub ready: bool,
    /// Which measurement method will be used, given what is available.
    pub measurement_method: String,
}

fn check(name: &str, ok: bool, detail: impl Into<String>, fix: Option<&str>) -> Check {
    Check {
        name: name.into(),
        ok,
        detail: detail.into(),
        fix: fix.map(|s| s.to_string()),
    }
}

fn compiler_version_args(name: ToolchainName) -> &'static [&'static str] {
    match name {
        ToolchainName::Gcc | ToolchainName::Gcc13 | ToolchainName::Gdc => {
            &["-dumpfullversion", "-dumpversion"]
        }
        ToolchainName::Clang | ToolchainName::Clang18 => &["-dumpversion"],
        ToolchainName::Rust => &["--version"],
        ToolchainName::Zig | ToolchainName::Odin => &["version"],
        ToolchainName::Go => &["version"],
    }
}

fn parse_compiler_version(name: ToolchainName, stdout: &str) -> Option<String> {
    match name {
        ToolchainName::Rust => stdout.split_whitespace().nth(1).map(str::to_string),
        ToolchainName::Go => stdout.split_whitespace().nth(2).map(str::to_string),
        ToolchainName::Odin => stdout.lines().next().map(|line| line.trim().to_string()),
        _ => stdout.lines().next().map(|line| line.trim().to_string()),
    }
}

pub fn run(format: Format) -> anyhow::Result<()> {
    let mut checks = Vec::new();
    let docker = Docker::default();

    let docker_ok = docker.available();
    checks.push(check(
        "docker",
        docker_ok,
        if docker_ok {
            "daemon reachable"
        } else {
            "daemon not reachable"
        },
        (!docker_ok).then_some("start it: `sudo dockerd &` or `sudo systemctl start docker`"),
    ));

    // Pinned images, by digest.
    for name in [
        ToolchainName::Gcc,
        ToolchainName::Gcc13,
        ToolchainName::Clang,
        ToolchainName::Clang18,
        ToolchainName::Rust,
        ToolchainName::Gdc,
        ToolchainName::Zig,
        ToolchainName::Odin,
        ToolchainName::Go,
    ] {
        match toolchain::pinned(name) {
            Some(tc) => {
                let image_ref = format!("{}@{}", tc.image, tc.image_digest);
                let present = docker_ok
                    && docker
                        .run(&["image".into(), "inspect".into(), image_ref.clone()])
                        .map(|(s, _, _)| s == 0)
                        .unwrap_or(false);
                checks.push(check(
                    &format!("image {name:?}"),
                    present,
                    if present {
                        format!("{} image present", tc.image_digest)
                    } else {
                        format!("{} image not built locally", tc.image_digest)
                    },
                    (!present).then_some("run `scripts/pin-toolchains.sh`"),
                ));
                let mut compiler_command = vec![
                    "run".to_string(),
                    "--rm".to_string(),
                    "--entrypoint".to_string(),
                    tc.compiler.clone(),
                    image_ref.clone(),
                ];
                compiler_command.extend(
                    compiler_version_args(name)
                        .iter()
                        .map(|argument| (*argument).to_string()),
                );
                let compiler_version = docker_ok
                    .then(|| docker.run(&compiler_command).ok())
                    .flatten()
                    .and_then(|(status, stdout, _)| {
                        (status == 0)
                            .then(|| parse_compiler_version(name, &stdout))
                            .flatten()
                    });
                let mut fingerprint_command = vec![
                    "run".to_string(),
                    "--rm".to_string(),
                    "--entrypoint".to_string(),
                    "sha256sum".to_string(),
                    image_ref.clone(),
                ];
                fingerprint_command.extend(tc.compiler_binaries.iter().cloned());
                let compiler_fingerprint = docker_ok
                    .then(|| docker.run(&fingerprint_command).ok())
                    .flatten()
                    .and_then(|(status, stdout, _)| {
                        (status == 0).then(|| ccx_core::capsule::sha256_hex(stdout.as_bytes()))
                    });
                let version_matches = compiler_version.as_deref() == Some(tc.version.as_str());
                let fingerprint_matches =
                    compiler_fingerprint.as_deref() == Some(tc.compiler_sha256.as_str());
                let compiler_matches = version_matches && fingerprint_matches;
                checks.push(check(
                    &format!("compiler {}", name.profile_id()),
                    compiler_matches,
                    match (compiler_version.as_deref(), compiler_fingerprint.as_deref()) {
                        (Some(version), Some(fingerprint))
                            if version_matches && fingerprint_matches =>
                        {
                            format!(
                                "pinned compiler version {version} and binary fingerprint {fingerprint} verified"
                            )
                        }
                        (Some(version), Some(fingerprint)) => format!(
                            "found version {version} and binary fingerprint {fingerprint}; lock requires version {} and fingerprint {}",
                            tc.version, tc.compiler_sha256
                        ),
                        _ => format!(
                            "{} or its pinned binary files are not available in the image",
                            tc.compiler
                        ),
                    },
                    (!compiler_matches)
                        .then_some("rebuild and repin the images with `scripts/pin-toolchains.sh`"),
                ));
                let expected_harness = toolchain::pinned_harness_fingerprint(&tc.image);
                let mut harness_command = vec![
                    "run".into(),
                    "--rm".into(),
                    "--entrypoint".into(),
                    "sha256sum".into(),
                    image_ref,
                ];
                harness_command.extend(
                    [
                        "/opt/ccx/runner",
                        "/opt/ccx/batch-runner",
                        "/opt/ccx/alloc_shim.o",
                    ]
                    .into_iter()
                    .map(str::to_string),
                );
                let actual_harness = docker_ok
                    .then(|| docker.run(&harness_command).ok())
                    .flatten()
                    .and_then(|(status, stdout, _)| {
                        (status == 0).then(|| ccx_core::capsule::sha256_hex(stdout.as_bytes()))
                    });
                let harness_ready = matches!(
                    (&actual_harness, &expected_harness),
                    (Some(actual), Some(expected)) if actual == expected
                );
                checks.push(check(
                    &format!("harness {name:?}"),
                    harness_ready,
                    match (&actual_harness, &expected_harness) {
                        (Some(actual), Some(expected)) if actual == expected => {
                            format!("runner and allocation shim fingerprint {actual} verified")
                        }
                        (Some(actual), Some(expected)) => format!(
                            "runner fingerprint {actual} differs from locked fingerprint {expected}"
                        ),
                        _ => "measurement runner, batch runner, or allocation shim fingerprint is unavailable".into(),
                    },
                    (!harness_ready)
                        .then_some("rebuild the pinned images with `scripts/pin-toolchains.sh`"),
                ));
            }
            None => checks.push(check(
                &format!("image {name:?}"),
                false,
                "not pinned in docker/toolchains.lock.json",
                Some("run `scripts/pin-toolchains.sh`"),
            )),
        }
    }

    for (name, binary) in [("cmake", "cmake"), ("ninja", "ninja"), ("make", "make")] {
        let expected = toolchain::pinned_build_tool(name);
        let gcc = toolchain::pinned(ToolchainName::Gcc);
        let probe = match (&expected, &gcc, docker_ok) {
            (Some(_), Some(gcc), true) => docker.run(&[
                "run".into(),
                "--rm".into(),
                "--entrypoint".into(),
                binary.into(),
                format!("{}@{}", gcc.image, gcc.image_digest),
                "--version".into(),
            ]),
            _ => Err(ccx_core::engine::EngineError::Other(
                "toolchain image or tool lock missing".into(),
            )),
        };
        let actual = probe.ok().and_then(|(status, stdout, _)| {
            if status == 0 {
                stdout
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().last())
                    .map(str::to_string)
            } else {
                None
            }
        });
        let hash_probe = match (&expected, &gcc, docker_ok) {
            (Some(_), Some(gcc), true) => docker.run(&[
                "run".into(),
                "--rm".into(),
                "--entrypoint".into(),
                "sha256sum".into(),
                format!("{}@{}", gcc.image, gcc.image_digest),
                format!("/usr/bin/{binary}"),
            ]),
            _ => Err(ccx_core::engine::EngineError::Other(
                "toolchain image or tool lock missing".into(),
            )),
        };
        let actual_hash = hash_probe.ok().and_then(|(status, stdout, _)| {
            if status == 0 {
                stdout.split_whitespace().next().map(str::to_string)
            } else {
                None
            }
        });
        let version = expected.as_ref().map(|tool| tool.version.as_str());
        let expected_hash = expected.as_ref().map(|tool| tool.binary_sha256.as_str());
        let version_matches = actual.as_deref() == version;
        let hash_matches = actual_hash.as_deref() == expected_hash;
        let present = version_matches && hash_matches;
        checks.push(check(
            &format!("{name} build tool"),
            present,
            match (
                actual.as_deref(),
                version,
                actual_hash.as_deref(),
                expected_hash,
            ) {
                (Some(found), Some(wanted), Some(found_hash), Some(wanted_hash))
                    if found == wanted && found_hash == wanted_hash =>
                {
                    format!("pinned version {found} and binary hash verified")
                }
                (Some(found), Some(wanted), Some(found_hash), Some(wanted_hash)) => format!(
                    "found version {found} (lock requires {wanted}); binary hash {found_hash} (lock requires {wanted_hash})"
                ),
                _ => "not available in the pinned GCC image or missing from the lock".into(),
            },
            (!present).then_some("run `scripts/pin-toolchains.sh` to rebuild the pinned images"),
        ));
    }

    // Valgrind: the source of deterministic counters when perf is unavailable.
    let valgrind_ok = docker_ok
        && toolchain::pinned(ToolchainName::Gcc)
            .map(|tc| {
                docker
                    .run(&[
                        "run".into(),
                        "--rm".into(),
                        "--entrypoint".into(),
                        "valgrind".into(),
                        format!("{}@{}", tc.image, tc.image_digest),
                        "--version".into(),
                    ])
                    .map(|(s, _, _)| s == 0)
                    .unwrap_or(false)
            })
            .unwrap_or(false);
    checks.push(check(
        "valgrind (cachegrind)",
        valgrind_ok,
        if valgrind_ok {
            "available: simulated counters can be measured"
        } else {
            "not available in the pinned image"
        },
        (!valgrind_ok).then_some("rebuild the image: `scripts/pin-toolchains.sh`"),
    ));

    // Hardware counters: a bonus when the kernel allows them, never required.
    let paranoid = read_perf_event_paranoid();
    let hardware = probe_hardware_counters(&docker, docker_ok);
    let perf_ok = hardware.as_ref().is_some_and(|counters| {
        counters.instructions.is_some()
            || counters.cache_misses.is_some()
            || counters.branch_misses.is_some()
            || counters.cpu_cycles.is_some()
    });
    let hardware_names = hardware.as_ref().map(|counters| {
        [
            ("instructions", counters.instructions),
            ("cache misses", counters.cache_misses),
            ("branch misses", counters.branch_misses),
            ("CPU cycles", counters.cpu_cycles),
        ]
        .into_iter()
        .filter_map(|(name, value)| value.map(|_| name))
        .collect::<Vec<_>>()
        .join(", ")
    });
    checks.push(check(
        "hardware counters (perf)",
        perf_ok,
        match (perf_ok, paranoid, hardware_names.as_deref()) {
            (true, Some(value), Some(names)) => {
                format!("perf_event_open works in the sandbox (perf_event_paranoid={value}); measured: {names}")
            }
            (true, None, Some(names)) => {
                format!("perf_event_open works in the sandbox; measured: {names}")
            }
            (false, Some(value), _) => {
                format!("perf_event_open unavailable in the sandbox (perf_event_paranoid={value})")
            }
            (false, None, _) => "perf_event_open unavailable in the sandbox".to_string(),
            (true, _, None) => "perf_event_open returned hardware counters in the sandbox".to_string(),
        },
        (!perf_ok).then_some("optional: Cachegrind simulated counters remain available"),
    ));

    let ready = checks
        .iter()
        .filter(|c| c.name != "hardware counters (perf)")
        .all(|c| c.ok);

    let measurement_method = if valgrind_ok {
        if perf_ok {
            format!(
                "cachegrind (deterministic, simulated) plus {} (measured, perf_event_open)",
                hardware_names.unwrap_or_default()
            )
        } else {
            "cachegrind (deterministic, simulated); hardware counters unavailable in this sandbox"
                .to_string()
        }
    } else {
        "none: no deterministic counter source available".to_string()
    };

    let report = DoctorReport {
        checks,
        ready,
        measurement_method,
    };

    cmd::emit(
        format,
        || {
            for c in &report.checks {
                let mark = if c.ok { "ok  " } else { "MISS" };
                println!("  {mark} {:<26} {}", c.name, c.detail);
                if let Some(fix) = &c.fix {
                    if !c.ok {
                        println!("       fix: {fix}");
                    }
                }
            }
            println!();
            println!("  measurement: {}", report.measurement_method);
            if report.ready {
                println!("  ready: the engine can build, run, and measure here");
            } else {
                println!("  not ready: resolve the MISS lines above");
            }
        },
        &report,
    )?;

    if !report.ready {
        return Err(Fixable::new("the environment is not ready")
            .fix("work through the MISS lines above")
            .into_error());
    }
    Ok(())
}

fn probe_hardware_counters(
    docker: &Docker,
    docker_ok: bool,
) -> Option<ccx_core::engine::HardwareCounters> {
    if !docker_ok {
        return None;
    }
    let tc = toolchain::pinned(ToolchainName::Gcc)?;
    let image = format!("{}@{}", tc.image, tc.image_digest);
    let args = [
        "run",
        "--rm",
        "--network",
        "none",
        "--read-only",
        "--memory",
        "128m",
        "--cpus",
        "1",
        "--pids-limit",
        "8",
        "--user",
        "65534:65534",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--entrypoint",
        "/opt/ccx/runner",
    ];
    let mut argv = args.into_iter().map(str::to_string).collect::<Vec<_>>();
    argv.extend([
        image,
        "--timeout-ms".into(),
        "1000".into(),
        "--memory-bytes".into(),
        "67108864".into(),
        "--stack-bytes".into(),
        "8388608".into(),
        "--pids-limit".into(),
        "8".into(),
        "--max-output-bytes".into(),
        "1024".into(),
        "--hardware".into(),
        "--".into(),
        "/bin/true".into(),
    ]);
    let (status, stdout, _) = docker.run(&argv).ok()?;
    if status != 0 {
        return None;
    }
    let outcome = parse_framed_report(stdout.as_bytes())?;
    outcome.hardware
}

#[cfg(test)]
mod tests {
    use super::parse_compiler_version;
    use ccx_core::capsule::ToolchainName;

    #[test]
    fn compiler_version_parser_matches_toolchain_output() {
        assert_eq!(
            parse_compiler_version(ToolchainName::Gcc13, "13.3.0\n"),
            Some("13.3.0".into())
        );
        assert_eq!(
            parse_compiler_version(ToolchainName::Rust, "rustc 1.85.1 (abc)\n"),
            Some("1.85.1".into())
        );
        assert_eq!(
            parse_compiler_version(ToolchainName::Go, "go version go1.26.10 linux/amd64\n"),
            Some("go1.26.10".into())
        );
        assert_eq!(
            parse_compiler_version(ToolchainName::Odin, "odin version dev-2026-10\n"),
            Some("odin version dev-2026-10".into())
        );
    }
}
