//! Toolchains and the compile flag allow-list.
//!
//! Layer 2 of Phase 1: builds are reproducible, so toolchains are pinned by
//! digest and the compiler is only ever handed flags from a known-good set.
//! Plugins, response files, and host-path includes are rejected outright: they
//! let a submission pull arbitrary code or files into the build.

use crate::capsule::{Language, ToolchainName};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A pinned compiler image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Toolchain {
    pub name: ToolchainName,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    #[serde(default)]
    pub image_id: Option<String>,
    #[serde(default)]
    pub environment_fingerprint: String,
    pub libc: String,
    /// Compiler binary invoked inside the image.
    pub compiler: String,
    pub compiler_binaries: Vec<String>,
    pub compiler_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct BuildTool {
    pub version: String,
    pub binary_sha256: String,
}

/// Why a flag was rejected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Rejected {
    pub flag: String,
    pub reason: String,
}

/// The result of checking a proposed flag list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlagCheck {
    pub accepted: Vec<String>,
    pub rejected: Vec<Rejected>,
}

impl FlagCheck {
    pub fn ok(&self) -> bool {
        self.rejected.is_empty()
    }
}

/// Flags that are always allowed, because they only affect code generation.
const ALLOWED_EXACT: &[&str] = &[
    "-O0",
    "-O1",
    "-O2",
    "-O3",
    "-Os",
    "-Oz",
    "-Ofast",
    "-g",
    "-g0",
    "-g1",
    "-g2",
    "-g3",
    "-Wall",
    "-Wextra",
    "-Wpedantic",
    "-Werror",
    "-std=c11",
    "-std=c17",
    "-std=c23",
    "-std=gnu11",
    "-std=gnu17",
    "-std=gnu23",
    "-std=c++14",
    "-std=c++17",
    "-std=c++20",
    "-std=c++23",
    "-std=gnu++17",
    "-std=gnu++20",
    "-march=native",
    "-mtune=native",
    "-mavx2",
    "-mavx512f",
    "-msse4.2",
    "-mfma",
    "-mpopcnt",
    "-mlzcnt",
    "-mbmi",
    "-mbmi2",
    "-ffast-math",
    "-fno-fast-math",
    "-fno-exceptions",
    "-fno-rtti",
    "-fomit-frame-pointer",
    "-fno-omit-frame-pointer",
    "-funroll-loops",
    "-fno-unroll-loops",
    "-flto",
    "-fno-lto",
    "-static",
    "-static-libgcc",
    "-static-libstdc++",
    "-pthread",
    "-no-pie",
    "-fno-pie",
    "-fno-stack-protector",
    "-fstack-protector-strong",
    "-pipe",
    "-c",
];

/// Flags allowed only with a validated value.
const ALLOWED_WITH_VALUE: &[&str] = &[
    "-I",
    "-L",
    "-l",
    "-D",
    "-U",
    "-march=",
    "-mtune=",
    "-mcpu=",
    "-std=",
    "-o",
    "-x",
    "-fmax-errors=",
    "-falign-functions=",
    "-falign-loops=",
    "--param",
];

/// Flags and constructs that are never allowed, with the reason.
fn forbidden(flag: &str) -> Option<&'static str> {
    if flag == "-fplugin" || flag.starts_with("-fplugin=") {
        return Some("compiler plugins can run arbitrary code at build time");
    }
    if flag == "-specs" || flag.starts_with("-specs=") {
        return Some("specs files redirect the compiler driver to arbitrary tooling");
    }
    if flag.starts_with('@') {
        return Some("response files can smuggle any flag past this check");
    }
    if flag == "-B" || flag.starts_with("-B") {
        return Some("-B redirects the compiler to attacker-controlled tools");
    }
    if flag.starts_with("-isystem") {
        return Some("-isystem can pull headers from host paths outside the image");
    }
    if flag.starts_with("-Wl,-rpath") || flag.starts_with("-Wl,--dynamic-linker") {
        return Some("rpath/dynamic-linker changes where the binary loads code from");
    }
    if flag.starts_with("-wrapper") || flag.starts_with("--wrapper") {
        return Some("wrappers execute arbitrary programs");
    }
    if flag.starts_with("/") || flag.contains("..") {
        return Some("absolute paths and parent traversal are not allowed in flags");
    }
    if flag == "-fprofile-generate" || flag.starts_with("-fprofile-generate=") {
        return Some("profile output writes to the host filesystem");
    }
    if flag.starts_with("-fprofile-use") {
        return Some("profile data would come from outside the experiment");
    }
    if flag == "-save-temps" || flag.starts_with("-save-temps=") {
        return Some("intermediate artefacts must stay inside the sandbox");
    }
    None
}

/// Validate a proposed flag list. Rejected flags are reported with a reason
/// rather than silently dropped, so the user learns what happened.
pub fn check_flags(flags: &[String]) -> FlagCheck {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();

    for flag in flags {
        if let Some(reason) = forbidden(flag) {
            rejected.push(Rejected {
                flag: flag.clone(),
                reason: reason.to_string(),
            });
            continue;
        }
        if ALLOWED_EXACT.contains(&flag.as_str()) {
            accepted.push(flag.clone());
            continue;
        }
        if let Some(prefix) = ALLOWED_WITH_VALUE
            .iter()
            .find(|p| flag.starts_with(**p) && flag.len() > p.len())
        {
            let value = &flag[prefix.len()..];
            if value.is_empty() {
                rejected.push(Rejected {
                    flag: flag.clone(),
                    reason: "flag value is empty".into(),
                });
            } else if value.starts_with('/') || value.contains("..") {
                // An absolute or parent-relative path escapes the sandbox and
                // lets the build read host files. Relative paths stay inside.
                rejected.push(Rejected {
                    flag: flag.clone(),
                    reason: "flag value must be a path inside the sandbox: absolute paths and \
                             parent traversal are not allowed"
                        .into(),
                });
            } else {
                accepted.push(flag.clone());
            }
            continue;
        }
        // A bare flag with a value supplied separately (e.g. `-o out`) is
        // handled by the caller splitting argv; anything else is unknown.
        if ALLOWED_WITH_VALUE.contains(&flag.as_str()) {
            accepted.push(flag.clone());
            continue;
        }
        rejected.push(Rejected {
            flag: flag.clone(),
            reason: "flag is not on the allow-list; add it deliberately if it is safe".into(),
        });
    }

    FlagCheck { accepted, rejected }
}

/// The pinned toolchains for Phase 1. Digests are filled in from
/// `docker/toolchains.lock.json`, which is generated by `scripts/pin-toolchains.sh`.
pub fn pinned(name: ToolchainName) -> Option<Toolchain> {
    let lock = include_str!("../../../docker/toolchains.lock.json");
    let parsed: ToolchainLock = serde_json::from_str(lock).ok()?;
    let environment_fingerprints = parsed.environment_fingerprints;
    let profile = parsed.toolchains.into_iter().find(|t| t.name == name)?;
    let expected_fingerprint = environment_fingerprints.get(&profile.image)?;
    (expected_fingerprint == &profile.environment_fingerprint
        && profile
            .environment_fingerprint
            .strip_prefix("sha256:")
            .is_some_and(|digest| {
                digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            }))
    .then_some(profile)
}

#[derive(Debug, Clone, Deserialize)]
struct ToolchainLock {
    toolchains: Vec<Toolchain>,
    #[serde(default)]
    harness_sha256_by_image: BTreeMap<String, String>,
    #[serde(default)]
    environment_fingerprints: BTreeMap<String, String>,
    #[serde(default)]
    build_tools: BTreeMap<String, BuildTool>,
}

pub fn pinned_harness_fingerprint(image: &str) -> Option<String> {
    let lock = include_str!("../../../docker/toolchains.lock.json");
    let parsed: ToolchainLock = serde_json::from_str(lock).ok()?;
    parsed.harness_sha256_by_image.get(image).cloned()
}

pub fn pinned_build_tool(name: &str) -> Option<BuildTool> {
    let lock = include_str!("../../../docker/toolchains.lock.json");
    let parsed: ToolchainLock = serde_json::from_str(lock).ok()?;
    parsed.build_tools.get(name).cloned()
}

/// Language to source-file extension.
pub fn extension(language: Language) -> &'static str {
    match language {
        Language::C => "c",
        Language::Cpp => "cpp",
        Language::Rust => "rs",
        Language::D => "d",
        Language::Zig => "zig",
        Language::Odin => "odin",
        Language::Go => "go",
        Language::AsmX86_64 => "s",
    }
}

/// The compiler driver for a toolchain and language.
///
/// A C++ translation unit must be linked with the C++ driver: `gcc` will
/// compile it but will not link libstdc++, and the failure appears at link time
/// as a pile of undefined `std::` symbols. Deriving the driver from the
/// language avoids making every caller remember that.
pub fn driver_for(toolchain: ToolchainName, language: Language) -> &'static str {
    match (toolchain, language) {
        (_, Language::Rust) => "rustc",
        (_, Language::Zig) => "zig",
        (_, Language::Odin) => "odin",
        (_, Language::Go) => "go",
        (ToolchainName::Zig, _) => "zig",
        (ToolchainName::Odin, _) => "odin",
        (ToolchainName::Go, _) => "go",
        (ToolchainName::Gdc, Language::D) => "gdc-14",
        (
            ToolchainName::Gcc
            | ToolchainName::Gcc13
            | ToolchainName::Clang
            | ToolchainName::Clang18,
            Language::D,
        ) => "gdc-14",
        (ToolchainName::Gcc, Language::Cpp) => "g++",
        (ToolchainName::Gcc, Language::C) => "gcc",
        (ToolchainName::Gcc, Language::AsmX86_64) => "gcc",
        (ToolchainName::Gcc13, Language::Cpp) => "g++-13",
        (ToolchainName::Gcc13, Language::C) => "gcc-13",
        (ToolchainName::Gcc13, Language::AsmX86_64) => "gcc-13",
        (ToolchainName::Clang, Language::Cpp) => "clang++",
        (ToolchainName::Clang, Language::C) => "clang",
        (ToolchainName::Clang, Language::AsmX86_64) => "clang",
        (ToolchainName::Clang18, Language::Cpp) => "clang++-18",
        (ToolchainName::Clang18, Language::C) => "clang-18",
        (ToolchainName::Clang18, Language::AsmX86_64) => "clang-18",
        (ToolchainName::Rust, _) => "rustc",
        (ToolchainName::Gdc, _) => "gdc-14",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(flags: &[&str]) -> FlagCheck {
        check_flags(&flags.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn ordinary_optimisation_flags_are_allowed() {
        let c = check(&["-O2", "-std=c++20", "-Wall", "-march=native"]);
        assert!(c.ok(), "rejected: {:?}", c.rejected);
        assert_eq!(c.accepted.len(), 4);
    }

    #[test]
    fn versioned_c_and_cpp_profiles_use_versioned_drivers() {
        assert_eq!(driver_for(ToolchainName::Gcc13, Language::C), "gcc-13");
        assert_eq!(driver_for(ToolchainName::Gcc13, Language::Cpp), "g++-13");
        assert_eq!(driver_for(ToolchainName::Clang18, Language::C), "clang-18");
        assert_eq!(
            driver_for(ToolchainName::Clang18, Language::Cpp),
            "clang++-18"
        );
        assert_eq!(ToolchainName::Gcc13.profile_id(), "gcc-13");
        assert_eq!(ToolchainName::Clang18.profile_id(), "clang-18");
    }

    #[test]
    fn d_uses_the_pinned_gdc_driver() {
        assert_eq!(extension(Language::D), "d");
        assert_eq!(driver_for(ToolchainName::Gdc, Language::D), "gdc-14");
    }

    #[test]
    fn zig_uses_the_pinned_zig_driver() {
        assert_eq!(extension(Language::Zig), "zig");
        assert_eq!(driver_for(ToolchainName::Zig, Language::Zig), "zig");
    }

    #[test]
    fn odin_uses_the_pinned_odin_driver() {
        assert_eq!(extension(Language::Odin), "odin");
        assert_eq!(driver_for(ToolchainName::Odin, Language::Odin), "odin");
    }

    #[test]
    fn go_uses_the_pinned_go_driver() {
        assert_eq!(extension(Language::Go), "go");
        assert_eq!(driver_for(ToolchainName::Go, Language::Go), "go");
    }

    #[test]
    fn cmake_generators_and_build_tools_are_pinned() {
        for name in ["cmake", "ninja", "make"] {
            let tool = pinned_build_tool(name).expect("build tool is pinned");
            assert!(!tool.version.is_empty());
            assert_eq!(tool.binary_sha256.len(), 64);
        }
    }

    #[test]
    fn supported_gcc_and_clang_versions_have_pinned_image_fingerprints() {
        for (name, version) in [
            (ToolchainName::Gcc, "14.2.0"),
            (ToolchainName::Gcc13, "13.3.0"),
            (ToolchainName::Clang, "19.1.7"),
            (ToolchainName::Clang18, "18.1.8"),
        ] {
            let profile = pinned(name).expect("compiler profile is pinned");
            assert_eq!(profile.version, version);
            assert!(profile.image_digest.starts_with("sha256:"));
            assert!(profile.environment_fingerprint.starts_with("sha256:"));
            assert_eq!(profile.compiler_sha256.len(), 64);
            assert!(!profile.compiler_binaries.is_empty());
        }
    }

    #[test]
    fn plugins_are_rejected() {
        let c = check(&["-fplugin=/tmp/evil.so"]);
        assert!(!c.ok());
        assert!(c.rejected[0].reason.contains("plugin"));
    }

    #[test]
    fn response_files_are_rejected() {
        let c = check(&["@flags.rsp"]);
        assert!(!c.ok());
        assert!(c.rejected[0].reason.contains("response file"));
    }

    #[test]
    fn host_path_includes_are_rejected() {
        for flag in ["-I/etc", "-I../../host", "/usr/include/x.h"] {
            let c = check(&[flag]);
            assert!(!c.ok(), "{flag} should be rejected");
        }
    }

    #[test]
    fn specs_and_wrappers_are_rejected() {
        for flag in ["-specs=evil", "--wrapper=/bin/sh", "-B/tmp/tools"] {
            let c = check(&[flag]);
            assert!(!c.ok(), "{flag} should be rejected");
        }
    }

    #[test]
    fn include_dir_inside_sandbox_is_allowed() {
        let c = check(&["-Iinclude", "-DNDEBUG", "-Llib", "-lm"]);
        assert!(c.ok(), "rejected: {:?}", c.rejected);
    }

    #[test]
    fn unknown_flags_are_reported_not_dropped() {
        let c = check(&["--totally-unknown"]);
        assert!(!c.ok());
        assert_eq!(c.rejected[0].flag, "--totally-unknown");
    }
}
