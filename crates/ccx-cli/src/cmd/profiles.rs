use crate::cmd;
use crate::Format;
use ccx_core::capsule::ToolchainName;
use ccx_core::toolchain;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ProfileCatalog {
    pub schema_version: u32,
    pub execution_targets: Vec<ExecutionTarget>,
    pub languages: Vec<LanguageProfile>,
    pub build_systems: Vec<BuildSystemProfile>,
    pub customisation: CustomisationProfile,
    pub metrics: Vec<MetricProfile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionTarget {
    pub os: String,
    pub architecture: String,
    pub available: bool,
    pub status: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LanguageProfile {
    pub language: String,
    pub available: bool,
    pub toolchains: Vec<ToolchainProfile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolchainProfile {
    pub id: String,
    pub compiler: String,
    pub version: Option<String>,
    pub image_digest: Option<String>,
    pub environment_fingerprint: Option<String>,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BuildSystemProfile {
    pub id: String,
    pub available_languages: Vec<String>,
    pub tools: Vec<String>,
    pub available: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CustomisationProfile {
    pub presets: Vec<String>,
    pub allowlisted_compiler_flags: bool,
    pub custom_program_arguments: bool,
    pub custom_compiler_argv: bool,
    pub custom_runtime_argv: bool,
    pub arbitrary_build_commands: bool,
    pub arbitrary_run_commands: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct MetricProfile {
    pub id: String,
    pub label: String,
    pub method: String,
    pub supported_languages: Vec<String>,
}

fn toolchain_id(name: ToolchainName) -> &'static str {
    name.profile_id()
}

fn toolchain_profile(name: ToolchainName) -> ToolchainProfile {
    match toolchain::pinned(name) {
        Some(profile) => ToolchainProfile {
            id: toolchain_id(name).into(),
            compiler: profile.compiler,
            version: Some(profile.version),
            image_digest: Some(profile.image_digest),
            environment_fingerprint: Some(profile.environment_fingerprint),
            available: true,
            reason: None,
        },
        None => ToolchainProfile {
            id: toolchain_id(name).into(),
            compiler: toolchain_id(name).into(),
            version: None,
            image_digest: None,
            environment_fingerprint: None,
            available: false,
            reason: Some("no pinned toolchain image is present in the lock".into()),
        },
    }
}

fn unavailable_toolchain(id: &str, compiler: &str, reason: &str) -> ToolchainProfile {
    ToolchainProfile {
        id: id.into(),
        compiler: compiler.into(),
        version: None,
        image_digest: None,
        environment_fingerprint: None,
        available: false,
        reason: Some(reason.into()),
    }
}

fn language_profile(language: &str, toolchains: Vec<ToolchainProfile>) -> LanguageProfile {
    let available = toolchains.iter().any(|profile| profile.available);
    LanguageProfile {
        language: language.into(),
        available,
        toolchains,
    }
}

pub fn catalog() -> ProfileCatalog {
    let build_tool_version = |name: &str| {
        toolchain::pinned_build_tool(name)
            .map(|tool| tool.version)
            .unwrap_or_else(|| "not pinned".into())
    };
    let cmake_version = build_tool_version("cmake");
    let ninja_version = build_tool_version("ninja");
    let make_version = build_tool_version("make");
    let has_cmake = toolchain::pinned_build_tool("cmake").is_some();
    let has_ninja = toolchain::pinned_build_tool("ninja").is_some();
    let has_make = toolchain::pinned_build_tool("make").is_some();
    let c_cpp_toolchains = [
        ToolchainName::Gcc,
        ToolchainName::Gcc13,
        ToolchainName::Clang,
        ToolchainName::Clang18,
    ];
    let has_c_cpp = c_cpp_toolchains
        .into_iter()
        .any(|name| toolchain::pinned(name).is_some());
    let mut cpp_languages = Vec::new();
    if has_c_cpp {
        cpp_languages.extend(["c".into(), "cpp".into()]);
    }
    let mut direct_languages = cpp_languages.clone();
    for (language, name) in [
        ("rust", ToolchainName::Rust),
        ("odin", ToolchainName::Odin),
        ("go", ToolchainName::Go),
        ("zig", ToolchainName::Zig),
        ("d", ToolchainName::Gdc),
    ] {
        if toolchain::pinned(name).is_some() {
            direct_languages.push(language.into());
        }
    }
    let has_linux_toolchain = !direct_languages.is_empty();
    let compiler_targets = vec![
        ExecutionTarget {
            os: "linux".into(),
            architecture: "x86_64".into(),
            available: has_linux_toolchain,
            status: if has_linux_toolchain {
                "pinned"
            } else {
                "unavailable"
            }
            .into(),
            reason: (!has_linux_toolchain)
                .then(|| "no Linux x86-64 toolchain image is pinned".into()),
        },
        ExecutionTarget {
            os: "linux".into(),
            architecture: "aarch64".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("no pinned ARM64 toolchain image and runner are available".into()),
        },
        ExecutionTarget {
            os: "linux".into(),
            architecture: "x86".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("no pinned 32-bit toolchain image and runner are available".into()),
        },
        ExecutionTarget {
            os: "windows".into(),
            architecture: "x86".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("no Windows 32-bit toolchain image and runner are available".into()),
        },
        ExecutionTarget {
            os: "windows".into(),
            architecture: "x86_64".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("MSVC and Windows execution images are not pinned".into()),
        },
        ExecutionTarget {
            os: "windows".into(),
            architecture: "aarch64".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("no Windows ARM64 toolchain image and runner are available".into()),
        },
        ExecutionTarget {
            os: "macos".into(),
            architecture: "x86".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("no macOS 32-bit toolchain image and runner are available".into()),
        },
        ExecutionTarget {
            os: "macos".into(),
            architecture: "x86_64".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("macOS execution images are not pinned".into()),
        },
        ExecutionTarget {
            os: "macos".into(),
            architecture: "aarch64".into(),
            available: false,
            status: "unavailable".into(),
            reason: Some("macOS ARM64 execution images are not pinned".into()),
        },
    ];

    let mut c_cpp_toolchain_profiles = vec![
        toolchain_profile(ToolchainName::Gcc),
        toolchain_profile(ToolchainName::Gcc13),
        toolchain_profile(ToolchainName::Clang),
        toolchain_profile(ToolchainName::Clang18),
    ];
    c_cpp_toolchain_profiles.push(unavailable_toolchain(
        "msvc",
        "cl.exe",
        "MSVC requires a pinned Windows toolchain image and Windows sandbox runner",
    ));

    let languages = vec![
        language_profile("c", c_cpp_toolchain_profiles.clone()),
        language_profile("cpp", c_cpp_toolchain_profiles),
        language_profile("rust", vec![toolchain_profile(ToolchainName::Rust)]),
        language_profile("odin", vec![toolchain_profile(ToolchainName::Odin)]),
        language_profile("go", vec![toolchain_profile(ToolchainName::Go)]),
        language_profile("zig", vec![toolchain_profile(ToolchainName::Zig)]),
        language_profile("d", vec![toolchain_profile(ToolchainName::Gdc)]),
    ];

    ProfileCatalog {
        schema_version: 12,
        execution_targets: compiler_targets,
        languages,
        build_systems: vec![
            BuildSystemProfile {
                id: "direct".into(),
                available_languages: direct_languages,
                tools: Vec::new(),
                available: has_linux_toolchain,
                reason: (!has_linux_toolchain)
                    .then(|| "no pinned direct-build compiler profiles are available".into()),
            },
            BuildSystemProfile {
                id: "cmake".into(),
                available_languages: cpp_languages.clone(),
                tools: vec![
                    format!("cmake {cmake_version}"),
                    format!("ninja {ninja_version}"),
                ],
                available: has_c_cpp && has_cmake && has_ninja,
                reason: (!(has_c_cpp && has_cmake && has_ninja)).then(|| {
                    "CMake with Ninja requires pinned C/C++ compiler, CMake, and Ninja profiles"
                        .into()
                }),
            },
            BuildSystemProfile {
                id: "cmake_make".into(),
                available_languages: cpp_languages,
                tools: vec![
                    format!("cmake {cmake_version}"),
                    format!("make {make_version}"),
                ],
                available: has_c_cpp && has_cmake && has_make,
                reason: (!(has_c_cpp && has_cmake && has_make)).then(|| {
                    "CMake with Unix Makefiles requires pinned C/C++ compiler, CMake, and make profiles"
                        .into()
                }),
            },
            BuildSystemProfile {
                id: "custom".into(),
                available_languages: if has_c_cpp {
                    vec!["c".into(), "cpp".into()]
                } else {
                    Vec::new()
                },
                tools: vec!["selected pinned GCC or Clang compiler".into()],
                available: has_c_cpp,
                reason: (!has_c_cpp)
                    .then(|| "custom compiler argv requires a pinned C/C++ compiler profile".into()),
            },
        ],
        customisation: CustomisationProfile {
            presets: vec!["standard".into(), "custom".into()],
            allowlisted_compiler_flags: true,
            custom_program_arguments: true,
            custom_compiler_argv: true,
            custom_runtime_argv: true,
            arbitrary_build_commands: false,
            arbitrary_run_commands: false,
            reason: "Custom build and runtime argv templates are bounded and recorded for replay; they do not invoke a shell or replace the pinned compiler, output path, source set, or sandbox limits".into(),
        },
        metrics: vec![
            metric("instructions", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("cache_misses", "simulated", "Cachegrind, fixed I1/D1 CCX v1 model"),
            metric("branch_misses", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric(
                "instruction_cache_misses",
                "simulated",
                "Cachegrind, fixed I1/D1 CCX v1 model",
            ),
            metric("data_cache_misses", "simulated", "Cachegrind, fixed I1/D1 CCX v1 model"),
            metric("data_refs", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("data_reads", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("data_writes", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("branch_instructions", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("cache_miss_rate", "simulated", "Cachegrind, fixed I1/D1 CCX v1 model"),
            metric("branch_miss_rate", "simulated", "Cachegrind, fixed CCX v1 model"),
            metric("instructions_hw", "measured when available", "Linux perf_event_open; host-specific"),
            metric("cache_misses_hw", "measured when available", "Linux perf_event_open; host-specific"),
            metric("branch_misses_hw", "measured when available", "Linux perf_event_open; host-specific"),
            metric("cpu_cycles_hw", "measured when available", "Linux perf_event_open; host-specific"),
            metric("ipc_hw", "measured when available", "hardware instructions divided by CPU cycles"),
            metric("average_cpu_frequency_hw", "measured when available", "CPU cycles divided by user CPU time"),
            metric("allocations", "measured", "Valgrind Memcheck xtree, main process"),
            metric("allocated_bytes", "measured", "Valgrind Memcheck xtree, main process"),
            metric("binary_size", "measured", "filesystem stat"),
            metric("peak_memory", "indicative", "getrusage peak RSS"),
            metric("wall_clock", "indicative", "repeated fresh-process timing"),
            metric(
                "reference_wall_clock",
                "indicative",
                "alternating paired timing of the problem reference",
            ),
            metric(
                "paired_speedup_ratio",
                "indicative",
                "median of per-pair reference time divided by solution time; above 1 means the solution was faster",
            ),
            metric("stdout_bytes", "measured", "sandbox runner byte counter"),
            metric("stderr_bytes", "measured", "sandbox runner byte counter"),
            metric("user_cpu_time", "measured", "getrusage"),
            metric("system_cpu_time", "measured", "getrusage"),
            metric("minor_page_faults", "measured", "getrusage"),
            metric("major_page_faults", "measured", "getrusage"),
            metric("voluntary_context_switches", "measured", "getrusage"),
            metric("involuntary_context_switches", "measured", "getrusage"),
            metric("block_input_ops", "measured", "getrusage"),
            metric("block_output_ops", "measured", "getrusage"),
            metric(
                "build_wall_clock",
                "measured",
                "median of three byte-identical clean builds; runner monotonic clock with bootstrap CI95",
            ),
            metric(
                "build_user_cpu_time",
                "measured",
                "median of three byte-identical clean builds; getrusage with bootstrap CI95",
            ),
            metric(
                "build_system_cpu_time",
                "measured",
                "median of three byte-identical clean builds; getrusage with bootstrap CI95",
            ),
            metric("live_cpu_frequency", "unavailable", "not collected"),
            metric("network_latency", "unavailable", "network is disabled in the execution sandbox"),
            metric("file_io_latency", "unavailable", "latency histograms are not collected"),
        ],
    }
}

fn metric(id: &str, status: &str, method: &str) -> MetricProfile {
    let supported_languages = match id {
        "allocations" | "allocated_bytes" => vec!["c", "cpp", "rust"],
        "live_cpu_frequency" | "network_latency" | "file_io_latency" => Vec::new(),
        _ => vec!["c", "cpp", "rust", "odin", "go", "zig", "d"],
    };
    MetricProfile {
        id: id.into(),
        label: match id {
            "peak_memory" | "wall_clock" | "reference_wall_clock" | "paired_speedup_ratio" => {
                "indicative".into()
            }
            "live_cpu_frequency" | "network_latency" | "file_io_latency" => "unavailable".into(),
            _ => status.into(),
        },
        method: method.into(),
        supported_languages: supported_languages
            .into_iter()
            .map(str::to_string)
            .collect(),
    }
}

pub fn run(format: Format) -> anyhow::Result<()> {
    let report = catalog();
    cmd::emit(
        format,
        || {
            println!(
                "CodeCurrency execution profile catalog v{}",
                report.schema_version
            );
            println!("\nTargets");
            for target in &report.execution_targets {
                println!(
                    "  {:<8} {:<8} {:<12} {}",
                    target.os,
                    target.architecture,
                    target.status,
                    target.reason.as_deref().unwrap_or("")
                );
            }
            println!("\nLanguages and pinned compilers");
            for language in &report.languages {
                let profiles = language
                    .toolchains
                    .iter()
                    .map(|profile| {
                        format!(
                            "{} {} ({})",
                            profile.compiler,
                            profile.version.as_deref().unwrap_or("not pinned"),
                            if profile.available {
                                "pinned"
                            } else {
                                "unavailable"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                println!("  {:<12} {profiles}", language.language);
            }
            println!("\nBuild systems");
            for build in &report.build_systems {
                println!(
                    "  {:<12} {:<12} languages={} tools={} {}",
                    build.id,
                    if build.available {
                        "available"
                    } else {
                        "unavailable"
                    },
                    build.available_languages.join(","),
                    build.tools.join(","),
                    build.reason.as_deref().unwrap_or("")
                );
            }
            println!("\nMetrics");
            for metric in &report.metrics {
                println!(
                    "  {:<24} {:<22} {:<48} languages={}",
                    metric.id,
                    metric.label,
                    metric.method,
                    if metric.supported_languages.is_empty() {
                        "none".to_string()
                    } else {
                        metric.supported_languages.join(",")
                    }
                );
            }
        },
        &report,
    )
}

#[cfg(test)]
mod tests {
    use super::catalog;

    #[test]
    fn catalog_exposes_current_and_unavailable_profiles() {
        let report = catalog();
        assert_eq!(report.schema_version, 12);
        assert_eq!(report.languages.len(), 7);
        assert!(!report
            .languages
            .iter()
            .any(|language| language.language == "asm-x86_64"));
        assert!(report
            .execution_targets
            .iter()
            .any(|target| target.os == "linux"
                && target.architecture == "x86_64"
                && target.available));
        assert!(report
            .execution_targets
            .iter()
            .any(|target| target.os == "windows" && !target.available));
        for os in ["linux", "windows", "macos"] {
            for architecture in ["x86", "x86_64", "aarch64"] {
                assert!(report
                    .execution_targets
                    .iter()
                    .any(|target| { target.os == os && target.architecture == architecture }));
            }
        }
        assert!(report.languages.iter().any(|language| {
            language.language == "cpp"
                && language
                    .toolchains
                    .iter()
                    .any(|toolchain| toolchain.id == "clang")
                && language
                    .toolchains
                    .iter()
                    .any(|toolchain| toolchain.id == "gcc")
                && language
                    .toolchains
                    .iter()
                    .any(|toolchain| toolchain.id == "gcc-13" && toolchain.available)
                && language
                    .toolchains
                    .iter()
                    .any(|toolchain| toolchain.id == "clang-18" && toolchain.available)
                && language
                    .toolchains
                    .iter()
                    .filter(|toolchain| toolchain.available)
                    .all(|toolchain| {
                        toolchain
                            .environment_fingerprint
                            .as_deref()
                            .is_some_and(|fingerprint| fingerprint.starts_with("sha256:"))
                    })
                && language.toolchains.iter().any(|toolchain| {
                    toolchain.id == "msvc"
                        && !toolchain.available
                        && toolchain
                            .reason
                            .as_deref()
                            .is_some_and(|reason| reason.contains("Windows"))
                })
        }));
        assert!(report
            .metrics
            .iter()
            .any(|metric| metric.id == "network_latency" && metric.label == "unavailable"));
        for metric_id in ["allocations", "allocated_bytes"] {
            let allocation_metric = report
                .metrics
                .iter()
                .find(|metric| metric.id == metric_id)
                .unwrap();
            assert_eq!(
                allocation_metric.supported_languages,
                vec!["c", "cpp", "rust"]
            );
        }
        assert!(report.metrics.iter().all(|metric| {
            metric.id != "instructions"
                || metric.supported_languages == vec!["c", "cpp", "rust", "odin", "go", "zig", "d"]
        }));
        assert!(report.metrics.iter().any(|metric| {
            metric.id == "data_reads"
                && metric.label == "simulated"
                && metric.method == "Cachegrind, fixed CCX v1 model"
        }));
        assert!(report.metrics.iter().any(|metric| {
            metric.id == "stdout_bytes" && metric.method == "sandbox runner byte counter"
        }));
        assert!(report
            .build_systems
            .iter()
            .any(|profile| profile.id == "cmake_make"
                && profile.tools.iter().any(|tool| tool == "make 4.4.1")));
        assert!(report.customisation.custom_program_arguments);
        assert!(report.customisation.custom_compiler_argv);
        assert!(report.customisation.custom_runtime_argv);
        assert!(!report.customisation.arbitrary_build_commands);
        assert!(!report.customisation.arbitrary_run_commands);
        assert!(report
            .customisation
            .reason
            .contains("do not invoke a shell"));
        assert!(report
            .build_systems
            .iter()
            .any(|profile| profile.id == "custom" && profile.available));
    }

    #[test]
    fn cache_model_is_fixed_in_the_measurement_catalog() {
        let metric = catalog()
            .metrics
            .into_iter()
            .find(|metric| metric.id == "cache_misses")
            .expect("cache-miss metric is present");
        assert_eq!(metric.method, "Cachegrind, fixed I1/D1 CCX v1 model");
    }
}
