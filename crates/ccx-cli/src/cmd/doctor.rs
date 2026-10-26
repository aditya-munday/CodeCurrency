//! `ccx doctor` — report what the machine can and cannot do.
//!
//! Every result depends on the environment, so the environment is checked
//! first and stated plainly. Anything missing is reported with the fix, never
//! silently worked around.

use crate::cmd::{self, Fixable};
use crate::Format;
use ccx_core::capsule::ToolchainName;
use ccx_core::engine::Docker;
use ccx_core::measure::{perf_available, read_perf_event_paranoid};
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

pub fn run(format: Format) -> anyhow::Result<()> {
    let mut checks = Vec::new();
    let docker = Docker::default();

    let docker_ok = docker.available();
    checks.push(check(
        "docker",
        docker_ok,
        if docker_ok { "daemon reachable" } else { "daemon not reachable" },
        (!docker_ok).then_some("start it: `sudo dockerd &` or `sudo systemctl start docker`"),
    ));

    // Pinned images, by digest.
    for name in [ToolchainName::Gcc, ToolchainName::Clang] {
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
                        format!("{} {} present", tc.compiler, tc.version)
                    } else {
                        format!("{} {} not built locally", tc.compiler, tc.version)
                    },
                    (!present).then_some("run `scripts/pin-toolchains.sh`"),
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
    let perf_ok = perf_available(paranoid);
    checks.push(check(
        "hardware counters (perf)",
        perf_ok,
        match paranoid {
            Some(v) => format!("perf_event_paranoid={v}"),
            None => "perf_event_paranoid is not readable".to_string(),
        },
        (!perf_ok).then_some(
            "optional: simulated counters are used instead. Lower it with \
             `sudo sysctl kernel.perf_event_paranoid=1` to also get hardware counts",
        ),
    ));

    let ready = checks
        .iter()
        .filter(|c| c.name != "hardware counters (perf)")
        .all(|c| c.ok);

    let measurement_method = if valgrind_ok {
        if perf_ok {
            "cachegrind (deterministic) plus hardware counters".to_string()
        } else {
            "cachegrind (deterministic, simulated)".to_string()
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
