use crate::cmd::{self, Fixable};
use crate::context::{BuildSystem, CmakeGenerator, Solution};
use crate::Format;
use ccx_core::capsule::repeatability::RankingEligibility;
use ccx_core::capsule::{Language, ToolchainName};
use ccx_core::toolchain::pinned;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct MatrixReport {
    pub problem: String,
    pub size: String,
    pub seed: u64,
    pub input_sha256: String,
    pub profiles: Vec<MatrixProfile>,
    pub comparisons: Vec<MatrixComparison>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixProfile {
    pub toolchain: String,
    pub compiler_version: String,
    pub opt_level: String,
    pub build_system: String,
    pub cmake_generator: Option<String>,
    pub capsule_path: String,
    pub capsule_id: String,
    pub input_sha256: String,
    pub ranked_metric: String,
    pub rankability: RankingEligibility,
    pub ranked_value: Option<f64>,
    pub ranked_uncertainty: Option<f64>,
    pub metrics: Vec<super::measure::MetricSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatrixComparison {
    pub profile_a: String,
    pub profile_b: String,
    pub metric: String,
    pub delta_b_minus_a: Option<f64>,
    pub tolerance: Option<f64>,
    pub result: MatrixComparisonResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixComparisonResult {
    WithinNoise,
    ProfileALower,
    ProfileBLower,
    NotComparable,
}

pub fn run(
    format: Format,
    dir: &Path,
    toolchains: &[String],
    opt_levels: &[String],
    build_systems: &[String],
    cmake_generators: &[String],
    repeat: u32,
    size: &str,
    seed: u64,
    output_dir: Option<&Path>,
) -> anyhow::Result<()> {
    if repeat == 0 || repeat > 100 {
        return Err(Fixable::new(format!(
            "repeat count {repeat} is outside the supported range"
        ))
        .fix("choose a repeat count from 1 to 100")
        .into_error());
    }

    let solution = Solution::load(dir)?;
    let selected_toolchains = select_toolchains(&solution, toolchains)?;
    let selected_opt_levels =
        select_opt_levels(solution.language, &solution.opt_level, opt_levels)?;
    let selected_build_profiles = select_build_profiles(
        solution.language,
        solution.build_system,
        solution.cmake_generator,
        solution.root.join("CMakeLists.txt").is_file(),
        build_systems,
        cmake_generators,
    )?;
    let profile_count = selected_toolchains
        .len()
        .saturating_mul(selected_opt_levels.len())
        .saturating_mul(selected_build_profiles.len());
    if profile_count > 128 {
        return Err(Fixable::new(format!("matrix expands to {profile_count} profiles"))
            .fix("select at most 128 combinations of compilers, optimization levels, build systems, and CMake generators")
            .into_error());
    }
    let output_dir = resolve_output_dir(&solution.root, output_dir);
    std::fs::create_dir_all(&output_dir)?;

    let mut profiles = Vec::new();
    let mut matrix_input_sha256: Option<String> = None;
    for toolchain in selected_toolchains {
        let pinned_toolchain = pinned(toolchain).ok_or_else(|| {
            Fixable::new(format!("no pinned {toolchain:?} compiler is available"))
                .fix("run `ccx doctor` to list available pinned profiles")
                .into_error()
        })?;
        for opt_level in &selected_opt_levels {
            for (build_system, cmake_generator) in &selected_build_profiles {
                let mut profile_solution = solution.clone();
                profile_solution.toolchain_name = toolchain;
                profile_solution.opt_level = opt_level.clone();
                profile_solution.build_system = *build_system;
                if let Some(generator) = cmake_generator {
                    profile_solution.cmake_generator = *generator;
                }
                if *build_system == BuildSystem::Custom && profile_solution.build_command.is_empty()
                {
                    profile_solution.build_command = crate::context::default_build_command();
                }

                let build_system_id = build_system_id(*build_system);
                let generator_id = cmake_generator.map(cmake_generator_id).unwrap_or("none");
                let profile_id = format!(
                    "{}-{}-{}-{}",
                    toolchain_name(toolchain),
                    opt_level.trim_start_matches('-').to_ascii_lowercase(),
                    build_system_id,
                    generator_id
                );
                let pending_path =
                    output_dir.join(format!(".pending-{profile_id}-{}.json", std::process::id()));
                let mut report = super::measure::measure_solution(
                    &profile_solution,
                    repeat,
                    Some(&pending_path),
                    size,
                    seed,
                )
                .map_err(|error| {
                    anyhow::anyhow!("matrix profile {profile_id} failed: {error:#}")
                })?;
                if let Some(expected_input) = &matrix_input_sha256 {
                    if expected_input != &report.input_sha256 {
                        let _ = std::fs::remove_file(&pending_path);
                        return Err(Fixable::new(format!(
                            "profile {profile_id} generated different input bytes"
                        ))
                        .fix("matrix comparisons require identical generated input; check generator determinism")
                        .into_error());
                    }
                } else {
                    matrix_input_sha256 = Some(report.input_sha256.clone());
                }
                let capsule_path =
                    output_dir.join(format!("{profile_id}-{}.capsule.json", report.capsule_id));
                if capsule_path.exists() {
                    std::fs::remove_file(&pending_path)?;
                } else {
                    std::fs::rename(&pending_path, &capsule_path)?;
                }
                report.capsule_path = capsule_path.display().to_string();
                let ranked = report
                    .metrics
                    .iter()
                    .find(|metric| metric.name == report.ranked_metric);
                profiles.push(MatrixProfile {
                    toolchain: toolchain_name(toolchain).to_string(),
                    compiler_version: pinned_toolchain.version.clone(),
                    opt_level: opt_level.clone(),
                    build_system: build_system_id.to_string(),
                    cmake_generator: cmake_generator.map(cmake_generator_id).map(str::to_string),
                    capsule_path: report.capsule_path,
                    capsule_id: report.capsule_id,
                    input_sha256: report.input_sha256,
                    ranked_metric: report.ranked_metric,
                    rankability: report.rankability,
                    ranked_value: ranked.map(|metric| metric.value),
                    ranked_uncertainty: ranked.and_then(|metric| metric.uncertainty),
                    metrics: report.metrics,
                });
            }
        }
    }

    let comparisons = compare_profiles(&profiles);
    let report = MatrixReport {
        problem: solution.problem.meta.id.clone(),
        size: size.to_string(),
        seed,
        input_sha256: matrix_input_sha256.unwrap_or_default(),
        profiles,
        comparisons,
    };

    cmd::emit(
        format,
        || {
            println!(
                "{}: {} execution profiles, size {}, seed {}",
                report.problem,
                report.profiles.len(),
                report.size,
                report.seed
            );
            println!();
            println!("  shared input sha256 {}", report.input_sha256);
            println!(
                "  {:<8} {:<10} {:<8} {:<10} {:<8} {:<18} {:>16} {:<10}",
                "compiler",
                "version",
                "opt",
                "build",
                "generator",
                "ranked metric",
                "value",
                "interval"
            );
            for profile in &report.profiles {
                let value = profile
                    .ranked_value
                    .map(|value| format!("{value:.6e}"))
                    .unwrap_or_else(|| "unavailable".to_string());
                let uncertainty = profile
                    .ranked_uncertainty
                    .map(|value| format!("+/-{value:.2e}"))
                    .unwrap_or_else(|| "n/a".to_string());
                println!(
                    "  {:<8} {:<10} {:<8} {:<10} {:<8} {:<18} {:>16} {:<10}",
                    profile.toolchain,
                    profile.compiler_version,
                    profile.opt_level,
                    profile.build_system,
                    profile.cmake_generator.as_deref().unwrap_or("-"),
                    profile.ranked_metric,
                    value,
                    uncertainty
                );
                if !profile.rankability.eligible {
                    println!(
                        "    not rankable: {}",
                        profile
                            .rankability
                            .reason
                            .as_deref()
                            .unwrap_or("repeatability has not been established")
                    );
                }
                println!("    capsule {}", profile.capsule_path);
            }
            if !report.comparisons.is_empty() {
                println!("\n  pairwise ranked-metric comparisons");
                for comparison in &report.comparisons {
                    let delta = comparison
                        .delta_b_minus_a
                        .map(|value| format!("delta={value:.3e}"))
                        .unwrap_or_else(|| "delta=n/a".to_string());
                    let tolerance = comparison
                        .tolerance
                        .map(|value| format!("tolerance={value:.3e}"))
                        .unwrap_or_else(|| "tolerance=n/a".to_string());
                    println!(
                        "    {} vs {}: {:?} ({delta}, {tolerance})",
                        comparison.profile_a, comparison.profile_b, comparison.result
                    );
                }
            }
            println!("\n  Each capsule includes the full metric set and repeatability intervals.");
        },
        &report,
    )
}

fn select_build_profiles(
    language: Language,
    configured_system: BuildSystem,
    configured_generator: CmakeGenerator,
    has_cmake_lists: bool,
    requested_systems: &[String],
    requested_generators: &[String],
) -> anyhow::Result<Vec<(BuildSystem, Option<CmakeGenerator>)>> {
    let systems = if requested_systems.is_empty() {
        vec![configured_system]
    } else {
        requested_systems
            .iter()
            .map(|system| match system.as_str() {
                "direct" => Ok(BuildSystem::Direct),
                "cmake" => Ok(BuildSystem::Cmake),
                "custom" => Ok(BuildSystem::Custom),
                _ => Err(Fixable::new(format!("unknown build system {system:?}"))
                    .fix("choose direct, cmake, or custom")
                    .into_error()),
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    };
    let has_cmake = systems.contains(&BuildSystem::Cmake);
    if !requested_generators.is_empty() && !has_cmake {
        return Err(
            Fixable::new("CMake generators were selected without the CMake build system")
                .fix("include `--build-system cmake` or omit `--cmake-generator`")
                .into_error(),
        );
    }
    let generators = if requested_generators.is_empty() {
        vec![configured_generator]
    } else {
        requested_generators
            .iter()
            .map(|generator| match generator.as_str() {
                "ninja" => Ok(CmakeGenerator::Ninja),
                "make" => Ok(CmakeGenerator::UnixMakefiles),
                _ => Err(
                    Fixable::new(format!("unknown CMake generator {generator:?}"))
                        .fix("choose ninja or make")
                        .into_error(),
                ),
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    };

    let mut unique_systems = Vec::new();
    for system in systems {
        if unique_systems.contains(&system) {
            return Err(Fixable::new(format!(
                "build system {} was selected more than once",
                build_system_id(system)
            ))
            .fix("list each build system once")
            .into_error());
        }
        if matches!(system, BuildSystem::Cmake | BuildSystem::Custom)
            && !matches!(language, Language::C | Language::Cpp)
        {
            return Err(Fixable::new(format!(
                "{} matrix profiles only support C and C++",
                build_system_id(system)
            ))
            .fix("use direct builds for this language")
            .into_error());
        }
        if system == BuildSystem::Cmake && !has_cmake_lists {
            return Err(
                Fixable::new("the CMake matrix profile needs CMakeLists.txt")
                    .fix("add a CMake project or omit `--build-system cmake`")
                    .into_error(),
            );
        }
        unique_systems.push(system);
    }

    let mut unique_generators = Vec::new();
    for generator in generators {
        if unique_generators.contains(&generator) {
            return Err(Fixable::new(format!(
                "CMake generator {} was selected more than once",
                cmake_generator_id(generator)
            ))
            .fix("list each CMake generator once")
            .into_error());
        }
        unique_generators.push(generator);
    }

    let mut profiles = Vec::new();
    for system in unique_systems {
        if system == BuildSystem::Cmake {
            profiles.extend(
                unique_generators
                    .iter()
                    .copied()
                    .map(|generator| (system, Some(generator))),
            );
        } else {
            profiles.push((system, None));
        }
    }
    Ok(profiles)
}

fn build_system_id(system: BuildSystem) -> &'static str {
    match system {
        BuildSystem::Direct => "direct",
        BuildSystem::Cmake => "cmake",
        BuildSystem::Custom => "custom",
    }
}

fn cmake_generator_id(generator: CmakeGenerator) -> &'static str {
    match generator {
        CmakeGenerator::Ninja => "ninja",
        CmakeGenerator::UnixMakefiles => "make",
    }
}

fn select_toolchains(
    solution: &Solution,
    requested: &[String],
) -> anyhow::Result<Vec<ToolchainName>> {
    let selected = if requested.is_empty() {
        if matches!(
            solution.language,
            Language::C | Language::Cpp | Language::AsmX86_64
        ) {
            vec![
                ToolchainName::Gcc,
                ToolchainName::Gcc13,
                ToolchainName::Clang,
                ToolchainName::Clang18,
            ]
        } else {
            vec![solution.toolchain_name]
        }
    } else {
        requested
            .iter()
            .map(|name| parse_toolchain(name))
            .collect::<anyhow::Result<Vec<_>>>()?
    };

    let mut unique = Vec::new();
    for toolchain in selected {
        if unique.contains(&toolchain) {
            return Err(Fixable::new(format!(
                "toolchain {} was selected more than once",
                toolchain_name(toolchain)
            ))
            .fix("list each compiler profile once")
            .into_error());
        }
        let compatible = match solution.language {
            Language::C | Language::Cpp | Language::AsmX86_64 => {
                matches!(
                    toolchain,
                    ToolchainName::Gcc
                        | ToolchainName::Gcc13
                        | ToolchainName::Clang
                        | ToolchainName::Clang18
                )
            }
            Language::Rust => toolchain == ToolchainName::Rust,
            Language::D => toolchain == ToolchainName::Gdc,
            Language::Zig => toolchain == ToolchainName::Zig,
            Language::Odin => toolchain == ToolchainName::Odin,
            Language::Go => toolchain == ToolchainName::Go,
        };
        if !compatible {
            return Err(Fixable::new(format!(
                "{} is not a compiler profile for {:?}",
                toolchain_name(toolchain),
                solution.language
            ))
            .fix("choose a compiler profile supported for this language")
            .into_error());
        }
        unique.push(toolchain);
    }
    Ok(unique)
}

fn select_opt_levels(
    language: Language,
    configured: &str,
    requested: &[String],
) -> anyhow::Result<Vec<String>> {
    if language == Language::AsmX86_64 {
        if !requested.is_empty() {
            return Err(
                Fixable::new("optimization levels do not apply to preassembled sources")
                    .fix("omit --opt-level; the matrix will compare the selected assemblers")
                    .into_error(),
            );
        }
        return Ok(vec!["none".to_string()]);
    }
    let selected = if requested.is_empty() {
        vec![configured.to_string()]
    } else {
        requested.to_vec()
    };
    let mut unique = Vec::new();
    for opt_level in selected {
        if !matches!(
            opt_level.as_str(),
            "-O0" | "-O1" | "-O2" | "-O3" | "-Os" | "-Oz" | "-Ofast"
        ) {
            return Err(
                Fixable::new(format!("unsupported optimization level {opt_level:?}"))
                    .fix("choose -O0, -O1, -O2, -O3, -Os, -Oz, or -Ofast")
                    .into_error(),
            );
        }
        if language == Language::Go && !matches!(opt_level.as_str(), "-O0" | "-O2") {
            return Err(Fixable::new(format!(
                "Go does not support optimization level {opt_level:?}"
            ))
            .fix("choose -O0 or -O2")
            .into_error());
        }
        if language == Language::Odin && !matches!(opt_level.as_str(), "-O0" | "-O2") {
            return Err(
                Fixable::new(format!("Odin supports the standard -O0 and -O2 profiles"))
                    .fix("choose -O0 or -O2")
                    .into_error(),
            );
        }
        if unique.contains(&opt_level) {
            return Err(Fixable::new(format!(
                "optimization level {opt_level:?} was selected more than once"
            ))
            .fix("list each optimization level once")
            .into_error());
        }
        unique.push(opt_level);
    }
    Ok(unique)
}

fn parse_toolchain(name: &str) -> anyhow::Result<ToolchainName> {
    match name {
        "gcc" => Ok(ToolchainName::Gcc),
        "gcc-13" => Ok(ToolchainName::Gcc13),
        "clang" => Ok(ToolchainName::Clang),
        "clang-18" => Ok(ToolchainName::Clang18),
        "rust" => Ok(ToolchainName::Rust),
        "gdc" => Ok(ToolchainName::Gdc),
        "zig" => Ok(ToolchainName::Zig),
        "odin" => Ok(ToolchainName::Odin),
        "go" => Ok(ToolchainName::Go),
        _ => Err(Fixable::new(format!("unknown toolchain {name:?}"))
            .fix("choose gcc, gcc-13, clang, clang-18, rust, gdc, zig, odin, or go")
            .into_error()),
    }
}

fn toolchain_name(toolchain: ToolchainName) -> &'static str {
    toolchain.profile_id()
}

fn resolve_output_dir(solution_root: &Path, requested: Option<&Path>) -> PathBuf {
    match requested {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| solution_root.join(path)),
        None => solution_root.join(".ccx/matrix"),
    }
}

fn compare_profiles(profiles: &[MatrixProfile]) -> Vec<MatrixComparison> {
    let mut comparisons = Vec::new();
    for index_a in 0..profiles.len() {
        for index_b in (index_a + 1)..profiles.len() {
            let profile_a = &profiles[index_a];
            let profile_b = &profiles[index_b];
            let comparable = profile_a.ranked_metric == profile_b.ranked_metric
                && profile_a.input_sha256 == profile_b.input_sha256
                && profile_a.ranked_value.is_some()
                && profile_b.ranked_value.is_some()
                && profile_a.rankability.eligible
                && profile_b.rankability.eligible;
            let delta = comparable
                .then(|| profile_b.ranked_value.unwrap() - profile_a.ranked_value.unwrap());
            let tolerance = comparable.then(|| {
                let uncertainty_a = profile_a.ranked_uncertainty.unwrap_or(0.0);
                let uncertainty_b = profile_b.ranked_uncertainty.unwrap_or(0.0);
                (2.0 * (uncertainty_a.powi(2) + uncertainty_b.powi(2)).sqrt()).max(0.5)
            });
            let result = match (delta, tolerance) {
                (Some(delta), Some(tolerance)) if delta.abs() <= tolerance => {
                    MatrixComparisonResult::WithinNoise
                }
                (Some(delta), _) if delta < 0.0 => MatrixComparisonResult::ProfileBLower,
                (Some(_), Some(_)) => MatrixComparisonResult::ProfileALower,
                _ => MatrixComparisonResult::NotComparable,
            };
            comparisons.push(MatrixComparison {
                profile_a: profile_label(profile_a),
                profile_b: profile_label(profile_b),
                metric: profile_a.ranked_metric.clone(),
                delta_b_minus_a: delta,
                tolerance,
                result,
            });
        }
    }
    comparisons
}

fn profile_label(profile: &MatrixProfile) -> String {
    let generator = profile
        .cmake_generator
        .as_ref()
        .map(|value| format!("/{value}"))
        .unwrap_or_default();
    format!(
        "{} {} {}{}",
        profile.toolchain, profile.opt_level, profile.build_system, generator
    )
}

#[cfg(test)]
mod tests {
    use super::{
        compare_profiles, parse_toolchain, select_build_profiles, select_opt_levels,
        toolchain_name, MatrixComparisonResult, MatrixProfile, RankingEligibility,
    };
    use crate::context::{BuildSystem, CmakeGenerator};
    use ccx_core::capsule::{Language, ToolchainName};

    fn rankability(eligible: bool) -> RankingEligibility {
        RankingEligibility {
            eligible,
            tolerance_pct: 0.01,
            observed_spread_pct: Some(if eligible { 0.0 } else { 1.0 }),
            reason: (!eligible).then(|| "observed spread exceeds tolerance".into()),
        }
    }

    #[test]
    fn parses_pinned_compiler_profiles() {
        assert_eq!(parse_toolchain("gcc").unwrap(), ToolchainName::Gcc);
        assert_eq!(parse_toolchain("gcc-13").unwrap(), ToolchainName::Gcc13);
        assert_eq!(parse_toolchain("clang").unwrap(), ToolchainName::Clang);
        assert_eq!(parse_toolchain("clang-18").unwrap(), ToolchainName::Clang18);
        assert_eq!(parse_toolchain("gdc").unwrap(), ToolchainName::Gdc);
    }

    #[test]
    fn rejects_unavailable_compiler_profiles() {
        assert!(parse_toolchain("msvc").is_err());
        assert!(parse_toolchain("clang-17").is_err());
    }

    #[test]
    fn assembly_matrix_has_no_optimization_profile() {
        assert_eq!(
            select_opt_levels(Language::AsmX86_64, "-O2", &[]).unwrap(),
            vec!["none"]
        );
        assert!(select_opt_levels(Language::AsmX86_64, "-O2", &["-O3".into()]).is_err());
    }

    #[test]
    fn optimization_profiles_respect_language_support() {
        assert!(select_opt_levels(Language::Go, "-O2", &["-O3".into()]).is_err());
        assert_eq!(
            select_opt_levels(Language::Cpp, "-O2", &["-O2".into(), "-O3".into()]).unwrap(),
            vec!["-O2", "-O3"]
        );
    }

    #[test]
    fn build_matrix_expands_integrations_and_cmake_generators() {
        let profiles = select_build_profiles(
            Language::Cpp,
            BuildSystem::Direct,
            CmakeGenerator::Ninja,
            true,
            &["direct".into(), "cmake".into(), "custom".into()],
            &["ninja".into(), "make".into()],
        )
        .unwrap();
        assert_eq!(
            profiles,
            vec![
                (BuildSystem::Direct, None),
                (BuildSystem::Cmake, Some(CmakeGenerator::Ninja)),
                (BuildSystem::Cmake, Some(CmakeGenerator::UnixMakefiles)),
                (BuildSystem::Custom, None),
            ]
        );
    }

    #[test]
    fn build_matrix_rejects_incompatible_or_unavailable_profiles() {
        assert!(select_build_profiles(
            Language::Rust,
            BuildSystem::Direct,
            CmakeGenerator::Ninja,
            true,
            &["cmake".into()],
            &[],
        )
        .is_err());
        assert!(select_build_profiles(
            Language::Cpp,
            BuildSystem::Direct,
            CmakeGenerator::Ninja,
            false,
            &["cmake".into()],
            &[],
        )
        .is_err());
        assert!(select_build_profiles(
            Language::Cpp,
            BuildSystem::Direct,
            CmakeGenerator::Ninja,
            true,
            &["direct".into()],
            &["make".into()],
        )
        .is_err());
    }

    #[test]
    fn compiler_profile_names_are_capsule_safe() {
        assert_eq!(toolchain_name(ToolchainName::Clang), "clang");
        assert_eq!(toolchain_name(ToolchainName::Gcc), "gcc");
    }

    #[test]
    fn profile_comparison_reports_values_within_repeatability_noise_as_tied() {
        let mut profiles = vec![
            MatrixProfile {
                toolchain: "gcc".into(),
                compiler_version: "14.2.0".into(),
                opt_level: "-O2".into(),
                build_system: "direct".into(),
                cmake_generator: None,
                capsule_path: "gcc.capsule.json".into(),
                capsule_id: "a".into(),
                input_sha256: "same-input".into(),
                ranked_metric: "instructions".into(),
                rankability: rankability(true),
                ranked_value: Some(100.0),
                ranked_uncertainty: Some(1.0),
                metrics: Vec::new(),
            },
            MatrixProfile {
                toolchain: "clang".into(),
                compiler_version: "19.1.7".into(),
                opt_level: "-O2".into(),
                build_system: "direct".into(),
                cmake_generator: None,
                capsule_path: "clang.capsule.json".into(),
                capsule_id: "b".into(),
                input_sha256: "same-input".into(),
                ranked_metric: "instructions".into(),
                rankability: rankability(true),
                ranked_value: Some(102.0),
                ranked_uncertainty: Some(1.0),
                metrics: Vec::new(),
            },
        ];

        let comparisons = compare_profiles(&profiles);
        assert_eq!(comparisons.len(), 1);
        assert_eq!(comparisons[0].result, MatrixComparisonResult::WithinNoise);
        profiles[0].rankability = rankability(false);
        assert_eq!(
            compare_profiles(&profiles)[0].result,
            MatrixComparisonResult::NotComparable
        );
    }

    #[test]
    fn profile_comparison_refuses_different_input_bytes() {
        let mut profiles = vec![
            MatrixProfile {
                toolchain: "gcc".into(),
                compiler_version: "14.2.0".into(),
                opt_level: "-O2".into(),
                build_system: "direct".into(),
                cmake_generator: None,
                capsule_path: "gcc.capsule.json".into(),
                capsule_id: "a".into(),
                input_sha256: "input-a".into(),
                ranked_metric: "instructions".into(),
                rankability: rankability(true),
                ranked_value: Some(100.0),
                ranked_uncertainty: Some(1.0),
                metrics: Vec::new(),
            },
            MatrixProfile {
                toolchain: "clang".into(),
                compiler_version: "19.1.7".into(),
                opt_level: "-O2".into(),
                build_system: "direct".into(),
                cmake_generator: None,
                capsule_path: "clang.capsule.json".into(),
                capsule_id: "b".into(),
                input_sha256: "input-b".into(),
                ranked_metric: "instructions".into(),
                rankability: rankability(true),
                ranked_value: Some(90.0),
                ranked_uncertainty: Some(0.0),
                metrics: Vec::new(),
            },
        ];

        let comparisons = compare_profiles(&profiles);
        assert_eq!(comparisons[0].result, MatrixComparisonResult::NotComparable);
        profiles[1].input_sha256 = profiles[0].input_sha256.clone();
        assert_eq!(
            compare_profiles(&profiles)[0].result,
            MatrixComparisonResult::ProfileBLower
        );
    }
}
