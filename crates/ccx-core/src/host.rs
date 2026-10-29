//! Host detection.
//!
//! The host fingerprint is recorded in every capsule so a result is always
//! labelled with where it came from. It must never influence deterministic
//! metrics: if it does, that is a bug in the metric, not a property of the
//! machine.

use crate::capsule::{host_fingerprint, Host};
#[cfg(target_os = "linux")]
use std::fs;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use std::process::Command;

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
    #[cfg(target_os = "linux")]
    {
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
    #[cfg(target_os = "macos")]
    {
        command_value("sysctl", &["-n", "machdep.cpu.brand_string"])
    }
    #[cfg(target_os = "windows")]
    {
        command_value(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-CimInstance Win32_Processor | Select-Object -First 1 -ExpandProperty Name)",
            ],
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown".to_string()
    }
}

fn os_pretty_name() -> String {
    #[cfg(target_os = "linux")]
    {
        if let Ok(text) = fs::read_to_string("/etc/os-release") {
            for line in text.lines() {
                if let Some(rest) = line.strip_prefix("PRETTY_NAME=") {
                    return rest.trim().trim_matches('"').to_string();
                }
            }
        }
        "unknown".to_string()
    }
    #[cfg(target_os = "macos")]
    {
        format!("macOS {}", command_value("sw_vers", &["-productVersion"]))
    }
    #[cfg(target_os = "windows")]
    {
        command_value(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-CimInstance Win32_OperatingSystem | Select-Object -ExpandProperty Caption)",
            ],
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        "unknown".to_string()
    }
}

fn uname_r() -> String {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/sys/kernel/osrelease")
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(target_os = "macos")]
    {
        command_value("uname", &["-r"])
    }
    #[cfg(target_os = "windows")]
    {
        command_value(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Environment]::OSVersion.Version.ToString()",
            ],
        )
    }
}

fn frequency_policy() -> String {
    #[cfg(target_os = "linux")]
    {
        let path = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";
        fs::read_to_string(path)
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        "unknown".to_string()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn command_value(program: &str, args: &[&str]) -> String {
    Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|result| result.status.success())
        .map(|result| String::from_utf8_lossy(&result.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_detection_records_platform_identity() {
        let host = detect();
        assert!(!host.arch.is_empty());
        assert_ne!(host.os, "unknown");
        assert_ne!(host.kernel, "unknown");
        assert!(host.nproc > 0);
        assert_eq!(
            host.fingerprint,
            host_fingerprint(
                &host.cpu_model,
                &host.arch,
                &host.os,
                &host.kernel,
                host.nproc
            )
        );
    }
}
