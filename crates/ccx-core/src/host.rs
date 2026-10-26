//! Host detection.
//!
//! The host fingerprint is recorded in every capsule so a result is always
//! labelled with where it came from. It must never influence deterministic
//! metrics: if it does, that is a bug in the metric, not a property of the
//! machine.

use crate::capsule::{host_fingerprint, Host};
use std::fs;

pub fn detect() -> Host {
    let cpu_model = cpu_model();
    let arch = std::env::consts::ARCH.to_string();
    let os = os_pretty_name();
    let kernel = uname_r();
    let nproc = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1);
    let frequency_policy = frequency_policy();
    let fingerprint = host_fingerprint(&cpu_model, &arch, &os, &kernel, nproc);

    Host {
        cpu_model,
        arch,
        os,
        kernel,
        nproc,
        frequency_policy,
        fingerprint,
    }
}

fn cpu_model() -> String {
    if let Ok(text) = fs::read_to_string("/proc/cpuinfo") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("model name") {
                if let Some((_, v)) = rest.split_once(':') {
                    return v.trim().to_string();
                }
            }
        }
    }
    "unknown".to_string()
}

fn os_pretty_name() -> String {
    if let Ok(text) = fs::read_to_string("/etc/os-release") {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
                return rest.trim().trim_matches('"').to_string();
            }
        }
    }
    "unknown".to_string()
}

fn uname_r() -> String {
    fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}

fn frequency_policy() -> String {
    let path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
    fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
