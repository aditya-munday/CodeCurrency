//! Shared command context: locating a solution, loading its problem, and
//! talking to Docker.
//!
//! Every command needs the same three things — the solution directory, the
//! problem it targets, and the pinned toolchain — so they are resolved once
//! here. This is also the single place that decides what a "solution folder"
//! looks like on disk.

use crate::cmd::Fixable;
use ccx_core::capsule::Host;
use ccx_core::capsule::{Language, ToolchainName};
use ccx_core::engine::Docker;
use ccx_core::problem::Problem;
use ccx_core::sandbox::SandboxPolicy;
use ccx_core::toolchain::{self, Toolchain};
use std::path::{Path, PathBuf};

/// The name of the file that marks a directory as a CodeCurrency solution.
pub const SOLUTION_MARKER: &str = "ccx.toml";

/// A loaded solution: its folder, its source, and the problem it targets.
#[derive(Debug, Clone)]
pub struct Solution {
    pub root: PathBuf,
    pub problem: Problem,
    /// Source files, relative path to contents, hashed into the capsule.
    pub sources: std::collections::BTreeMap<String, String>,
    pub project_files: std::collections::BTreeMap<String, String>,
    pub language: Language,
    pub toolchain_name: ToolchainName,
    pub opt_level: String,
    pub compiler_flags: Vec<String>,
    pub program_args: Vec<String>,
    pub build_command: Vec<String>,
    pub run_command: Vec<String>,
    pub build_system: BuildSystem,
    pub cmake_generator: CmakeGenerator,
    pub cmake_target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildPreset {
    Standard,
    Custom,
}

impl Default for BuildPreset {
    fn default() -> Self {
        Self::Standard
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildSystem {
    Direct,
    Cmake,
    Custom,
}

pub fn default_build_command() -> Vec<String> {
    ["{compiler}", "{flags}", "-o", "{output}", "{sources}"]
        .into_iter()
        .map(str::to_string)
        .collect()
}

pub fn validate_build_command(
    language: Language,
    build_system: BuildSystem,
    command: &[String],
) -> Result<(), String> {
    if build_system != BuildSystem::Custom {
        return if command.is_empty() {
            Ok(())
        } else {
            Err("build_command is only supported with build_system = \"custom\"".into())
        };
    }
    if !matches!(language, Language::C | Language::Cpp) {
        return Err("custom compiler commands are currently available for C and C++".into());
    }
    let default = default_build_command();
    let reordered = ["{compiler}", "{sources}", "{flags}", "-o", "{output}"]
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    if command != default && command != reordered {
        return Err("custom build_command must use {compiler}, {flags}, -o, {output}, and {sources}; only source and flag ordering can change".into());
    }
    Ok(())
}

pub fn validate_run_command(command: &[String]) -> Result<(), String> {
    if command.is_empty() {
        return Ok(());
    }
    if command.len() > 64 {
        return Err("run_command exceeds 64 argv entries".into());
    }
    let mut total_bytes = 0usize;
    let mut binary_positions = Vec::new();
    let mut args_positions = Vec::new();
    for (index, argument) in command.iter().enumerate() {
        if argument.is_empty() || argument.len() > 4096 || argument.contains('\0') {
            return Err(
                "run_command entries must be non-empty, at most 4096 bytes, and cannot contain NUL"
                    .into(),
            );
        }
        total_bytes = total_bytes.saturating_add(argument.len());
        match argument.as_str() {
            "{binary}" => binary_positions.push(index),
            "{args}" => args_positions.push(index),
            _ if argument.contains('{') || argument.contains('}') => {
                return Err(format!("unsupported run_command placeholder {argument:?}"));
            }
            _ => {}
        }
    }
    if total_bytes > 16_384 {
        return Err("run_command exceeds 16384 total bytes".into());
    }
    if binary_positions.len() != 1 || args_positions.len() != 1 {
        return Err("run_command must contain {binary} and {args} exactly once".into());
    }
    if binary_positions[0] >= args_positions[0] {
        return Err("run_command must place {binary} before {args}".into());
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CmakeGenerator {
    Ninja,
    UnixMakefiles,
}

impl Default for CmakeGenerator {
    fn default() -> Self {
        Self::Ninja
    }
}

pub fn default_cmake_target() -> String {
    "ccx_solution".into()
}

pub fn validate_cmake_target(target: &str) -> Result<(), String> {
    let mut chars = target.chars();
    let valid_first = chars
        .next()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    let valid_rest =
        chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '+'));
    if target.len() > 128
        || !valid_first
        || !valid_rest
        || matches!(
            target,
            "all" | "clean" | "help" | "install" | "test" | "package" | "package_source"
        )
    {
        return Err("cmake_target must name an executable target using at most 128 ASCII letters, digits, underscores, dots, plus signs, or hyphens; CMake built-in targets are not allowed".into());
    }
    Ok(())
}

impl Default for BuildSystem {
    fn default() -> Self {
        Self::Direct
    }
}

impl Solution {
    /// Discover and load the solution rooted at `dir`.
    pub fn load(dir: &Path) -> anyhow::Result<Solution> {
        let root = dir.canonicalize().map_err(|e| {
            Fixable::new(format!("cannot open {}: {e}", dir.display()))
                .fix("check the path exists")
                .into_error()
        })?;

        let marker = root.join(SOLUTION_MARKER);
        let text = std::fs::read_to_string(&marker).map_err(|_| {
            Fixable::new(format!("{} is not a CodeCurrency solution", root.display()))
                .fix("run `ccx init` here, or pass --dir pointing at a solution folder")
                .fix(format!("a solution folder contains a {SOLUTION_MARKER}"))
                .into_error()
        })?;

        let cfg: SolutionConfig = toml::from_str(&text).map_err(|e| {
            Fixable::new(format!("{SOLUTION_MARKER} is malformed: {e}"))
                .fix("see `ccx init` output for the expected shape")
                .into_error()
        })?;

        let problem_root = root.join(&cfg.problem);
        let problem = Problem::load(&problem_root).map_err(|e| {
            Fixable::new(format!(
                "problem at {} failed to load: {e}",
                problem_root.display()
            ))
            .fix("check the problem folder has problem.json, generators, and solutions")
            .fix("run `ccx validate <problem-folder>` for a full report")
            .into_error()
        })?;

        let language = cfg.language().map_err(|e| {
            Fixable::new(format!("invalid language in {SOLUTION_MARKER}: {e}"))
                .fix("currently supported build languages are `c`, `cpp`, `rust`, `d`, `zig`, `odin`, and `go`")
                .into_error()
        })?;
        let toolchain_name = cfg.toolchain(language).map_err(|e| {
            Fixable::new(format!("invalid toolchain in {SOLUTION_MARKER}: {e}"))
                .fix("currently supported toolchains are `gcc`, `clang`, `rust`, `gdc`, `zig`, `odin`, and `go`")
                .into_error()
        })?;
        let pinned_toolchain = toolchain::pinned(toolchain_name).ok_or_else(|| {
            Fixable::new(format!(
                "no pinned {toolchain_name:?} toolchain is available"
            ))
            .fix("choose a compiler profile listed by `ccx doctor`")
            .into_error()
        })?;
        if let Some(version) = &cfg.compiler_version {
            if version != &pinned_toolchain.version {
                return Err(Fixable::new(format!(
                    "compiler version {version:?} is not pinned for {:?}",
                    toolchain_name
                ))
                .fix(format!(
                    "the available pinned version is {:?}",
                    pinned_toolchain.version
                ))
                .fix("select a toolchain image that is pinned in docker/toolchains.lock.json")
                .into_error());
            }
        }
        validate_build_customization(
            language,
            cfg.build_preset,
            cfg.build_system,
            &cfg.compiler_flags,
        )
        .map_err(|message| {
            Fixable::new(message)
                .fix("use the standard preset or a bounded, allow-listed C/C++ flag list")
                .into_error()
        })?;
        let build_command =
            if cfg.build_system == BuildSystem::Custom && cfg.build_command.is_empty() {
                default_build_command()
            } else {
                cfg.build_command.clone()
            };
        validate_build_command(language, cfg.build_system, &build_command).map_err(|message| {
            Fixable::new(message)
                .fix("use a C/C++ compiler argv template with the pinned compiler, allow-listed flags, output, and source placeholders")
                .into_error()
        })?;
        validate_run_command(&cfg.run_command).map_err(|message| {
            Fixable::new(message)
                .fix("use a direct argv list with {binary} and {args}; tokens are passed without shell parsing")
                .into_error()
        })?;
        validate_program_args(&cfg.program_args).map_err(|message| {
            Fixable::new(message)
                .fix("use up to 64 bounded arguments; each argument is passed directly without a shell")
                .into_error()
        })?;
        if cfg.target_os != "linux" || cfg.target_arch != "x86_64" {
            return Err(Fixable::new(format!(
                "target {}/{} has no pinned execution image",
                cfg.target_os, cfg.target_arch
            ))
            .fix("the available end-to-end target is linux/x86_64; cross-target profiles need their own pinned image and runner")
            .into_error());
        }
        if cfg.build_system == BuildSystem::Cmake {
            validate_cmake_target(&cfg.cmake_target).map_err(|message| {
                Fixable::new(message)
                    .fix("set cmake_target to the executable target defined by CMakeLists.txt")
                    .into_error()
            })?;
            if !matches!(language, Language::C | Language::Cpp) {
                return Err(Fixable::new("CMake builds are currently supported for C and C++")
                    .fix("use `build_system = \"direct\"` for Rust, D, Zig, Odin, Go, or assembly")
                    .into_error());
            }
            if !root.join("CMakeLists.txt").is_file() {
                return Err(
                    Fixable::new("CMake build selected but CMakeLists.txt is missing")
                        .fix("create CMakeLists.txt or select `build_system = \"direct\"`")
                        .into_error(),
                );
            }
        }
        if !matches!(
            cfg.opt_level.as_str(),
            "-O0" | "-O1" | "-O2" | "-O3" | "-Os" | "-Oz" | "-Ofast"
        ) {
            return Err(Fixable::new(format!(
                "invalid opt_level in {SOLUTION_MARKER}: {:?}",
                cfg.opt_level
            ))
            .fix("choose one of -O0, -O1, -O2, -O3, -Os, -Oz, or -Ofast")
            .into_error());
        }
        if language == Language::Go && !matches!(cfg.opt_level.as_str(), "-O0" | "-O2") {
            return Err(Fixable::new(format!(
                "Go does not support opt_level {:?}",
                cfg.opt_level
            ))
            .fix("choose -O0 to disable compiler optimizations or -O2 for the Go compiler default")
            .into_error());
        }
        let ext = toolchain::extension(language);
        let mut sources = std::collections::BTreeMap::new();
        let mut project_files = std::collections::BTreeMap::new();
        // Every source file in the folder is part of the solution's identity.
        // A hidden directory (.ccx, .git) is build state, not source.
        collect_project_files(&root, &root, ext, &mut sources, &mut project_files)?;

        if sources.is_empty() {
            return Err(Fixable::new(format!(
                "no .{ext} source files found in {}",
                root.display()
            ))
            .fix(format!(
                "add a .{ext} file, or run `ccx init` to scaffold one"
            ))
            .into_error());
        }

        Ok(Solution {
            root,
            problem,
            sources,
            project_files,
            language,
            toolchain_name,
            opt_level: cfg.opt_level,
            compiler_flags: cfg.compiler_flags,
            program_args: cfg.program_args,
            build_command,
            run_command: cfg.run_command,
            build_system: cfg.build_system,
            cmake_generator: cfg.cmake_generator,
            cmake_target: cfg.cmake_target,
        })
    }

    pub fn toolchain(&self) -> anyhow::Result<Toolchain> {
        toolchain::pinned(self.toolchain_name).ok_or_else(|| {
            Fixable::new(format!(
                "no pinned {:?} toolchain in docker/toolchains.lock.json",
                self.toolchain_name
            ))
            .fix("run `scripts/pin-toolchains.sh` to build and pin the images")
            .into_error()
        })
    }

    /// The scratch directory the engine uses for build output.
    pub fn work_dir(&self) -> PathBuf {
        self.root.join(".ccx")
    }

    pub fn out_dir(&self) -> PathBuf {
        self.work_dir().join("out")
    }

    /// A solution view over a problem's own reference, used by `validate` to
    /// exercise a problem folder that has no user solution yet.
    pub fn for_problem(problem: &Problem) -> anyhow::Result<Solution> {
        let sources = std::collections::BTreeMap::from([(
            "main.cpp".to_string(),
            std::fs::read_to_string(problem.root.join("reference.cpp"))?,
        )]);
        let project_files = sources.clone();
        Ok(Solution {
            root: problem.root.clone(),
            problem: problem.clone(),
            sources,
            project_files,
            language: Language::Cpp,
            toolchain_name: ToolchainName::Gcc,
            opt_level: "-O2".to_string(),
            compiler_flags: Vec::new(),
            program_args: Vec::new(),
            build_command: Vec::new(),
            run_command: Vec::new(),
            build_system: BuildSystem::Direct,
            cmake_generator: CmakeGenerator::default(),
            cmake_target: default_cmake_target(),
        })
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.out_dir())?;
        Ok(())
    }
}

fn collect_project_files(
    root: &Path,
    dir: &Path,
    extension: &str,
    sources: &mut std::collections::BTreeMap<String, String>,
    project_files: &mut std::collections::BTreeMap<String, String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if name.starts_with('.')
                || matches!(name.as_ref(), "target" | "build" | "dist" | "node_modules")
            {
                continue;
            }
            collect_project_files(root, &path, extension, sources, project_files)?;
        } else if kind.is_file() {
            let relative = path.strip_prefix(root)?;
            let relative = relative.to_string_lossy().replace('\\', "/");
            if (name.starts_with("capsule")
                && path.extension().and_then(|value| value.to_str()) == Some("json"))
                || name.ends_with(".capsule.json")
            {
                continue;
            }
            let contents = std::fs::read_to_string(&path)?;
            if path.extension().and_then(|value| value.to_str()) == Some(extension) {
                sources.insert(relative.clone(), contents.clone());
            }
            project_files.insert(relative, contents);
        }
    }
    Ok(())
}

/// The `ccx.toml` shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SolutionConfig {
    /// Path to the problem folder, relative to the solution.
    pub problem: String,
    /// `cpp`, `c`, or `asm`.
    #[serde(default = "default_language")]
    pub language: String,
    /// `gcc` or `clang`.
    #[serde(default = "default_toolchain")]
    pub toolchain: String,
    /// Optimisation level for measured builds.
    #[serde(default = "default_opt")]
    pub opt_level: String,
    #[serde(default)]
    pub build_preset: BuildPreset,
    #[serde(default)]
    pub compiler_version: Option<String>,
    #[serde(default)]
    pub compiler_flags: Vec<String>,
    #[serde(default)]
    pub program_args: Vec<String>,
    #[serde(default)]
    pub build_command: Vec<String>,
    #[serde(default)]
    pub run_command: Vec<String>,
    #[serde(default = "default_target_os")]
    pub target_os: String,
    #[serde(default = "default_target_arch")]
    pub target_arch: String,
    #[serde(default)]
    pub build_system: BuildSystem,
    #[serde(default)]
    pub cmake_generator: CmakeGenerator,
    #[serde(default = "default_cmake_target")]
    pub cmake_target: String,
}

fn default_language() -> String {
    "cpp".into()
}

fn default_toolchain() -> String {
    "auto".into()
}

fn default_opt() -> String {
    "-O2".into()
}

fn default_target_os() -> String {
    "linux".into()
}

fn default_target_arch() -> String {
    "x86_64".into()
}

pub fn validate_program_args(args: &[String]) -> Result<(), String> {
    if args.len() > 64 {
        return Err("program argument list exceeds 64 entries".into());
    }
    let mut total_bytes = 0usize;
    for argument in args {
        if argument.len() > 4096 || argument.contains('\0') {
            return Err(
                "program arguments must be at most 4096 bytes and cannot contain NUL".into(),
            );
        }
        total_bytes = total_bytes.saturating_add(argument.len());
    }
    if total_bytes > 16_384 {
        return Err("program argument list exceeds 16384 total bytes".into());
    }
    Ok(())
}

pub fn validate_build_customization(
    language: Language,
    preset: BuildPreset,
    build_system: BuildSystem,
    flags: &[String],
) -> Result<(), String> {
    if preset == BuildPreset::Standard && !flags.is_empty() {
        return Err("compiler_flags require build_preset = \"custom\"".into());
    }
    if preset == BuildPreset::Custom {
        if !matches!(language, Language::C | Language::Cpp) {
            return Err("custom compiler flags are currently available for C and C++".into());
        }
        if flags.len() > 32 {
            return Err("custom compiler flag list exceeds 32 entries".into());
        }
        let forbidden_controls = ["-o", "-c", "-x", "--param", "-I", "-L", "-l", "-D", "-U"];
        let mut rejected = Vec::new();
        for flag in flags {
            if forbidden_controls.contains(&flag.as_str()) {
                rejected.push(format!(
                    "{flag}: values must be attached to the flag and output controls are reserved"
                ));
            } else if flag == "-march=native" || flag == "-mtune=native" {
                rejected.push(format!(
                    "{flag}: native tuning is not replayable across host CPUs"
                ));
            } else if flag.len() > 128
                || !flag.is_ascii()
                || flag.chars().any(|ch| {
                    !(ch.is_ascii_alphanumeric()
                        || matches!(ch, '-' | '_' | '.' | '/' | '=' | ',' | '+' | ':'))
                })
            {
                rejected.push(format!(
                    "{flag:?}: use only bounded ASCII compiler-flag characters"
                ));
            }
        }
        let checked = toolchain::check_flags(flags);
        rejected.extend(
            checked
                .rejected
                .iter()
                .map(|item| format!("{}: {}", item.flag, item.reason)),
        );
        if !rejected.is_empty() {
            return Err(format!(
                "custom compiler flags were rejected: {}",
                rejected.join("; ")
            ));
        }
        if build_system == BuildSystem::Cmake && flags.iter().any(|flag| flag.starts_with("-I")) {
            return Err("-I custom flags are not supported with CMake; declare include paths in CMakeLists.txt".into());
        }
    }
    Ok(())
}

impl SolutionConfig {
    pub fn language(&self) -> Result<Language, String> {
        match self.language.as_str() {
            "c" => Ok(Language::C),
            "cpp" | "c++" => Ok(Language::Cpp),
            "rust" => Ok(Language::Rust),
            "d" => Ok(Language::D),
            "zig" => Ok(Language::Zig),
            "odin" => Ok(Language::Odin),
            "go" => Ok(Language::Go),
            other => Err(format!("{other:?} is not supported by this build")),
        }
    }

    pub fn toolchain(&self, language: Language) -> Result<ToolchainName, String> {
        match self.toolchain.as_str() {
            "auto" if language == Language::Rust => Ok(ToolchainName::Rust),
            "auto" if language == Language::D => Ok(ToolchainName::Gdc),
            "auto" if language == Language::Zig => Ok(ToolchainName::Zig),
            "auto" if language == Language::Odin => Ok(ToolchainName::Odin),
            "auto" if language == Language::Go => Ok(ToolchainName::Go),
            "auto" => Ok(ToolchainName::Gcc),
            "gcc"
                if language != Language::Rust
                    && language != Language::D
                    && language != Language::Zig
                    && language != Language::Odin
                    && language != Language::Go =>
            {
                Ok(ToolchainName::Gcc)
            }
            "gcc-13"
                if language != Language::Rust
                    && language != Language::D
                    && language != Language::Zig
                    && language != Language::Odin
                    && language != Language::Go =>
            {
                Ok(ToolchainName::Gcc13)
            }
            "clang"
                if language != Language::Rust
                    && language != Language::D
                    && language != Language::Zig
                    && language != Language::Odin
                    && language != Language::Go =>
            {
                Ok(ToolchainName::Clang)
            }
            "clang-18"
                if language != Language::Rust
                    && language != Language::D
                    && language != Language::Zig
                    && language != Language::Odin
                    && language != Language::Go =>
            {
                Ok(ToolchainName::Clang18)
            }
            "rust" if language == Language::Rust => Ok(ToolchainName::Rust),
            "gdc" if language == Language::D => Ok(ToolchainName::Gdc),
            "zig" if language == Language::Zig => Ok(ToolchainName::Zig),
            "odin" if language == Language::Odin => Ok(ToolchainName::Odin),
            "go" if language == Language::Go => Ok(ToolchainName::Go),
            other => Err(format!("{other:?} is not supported by this build")),
        }
    }
}

/// Confirm Docker is usable, with a fixable error if it is not.
pub fn require_docker() -> anyhow::Result<Docker> {
    let docker = Docker::default();
    if !docker.available() {
        return Err(Fixable::new("Docker is not available")
            .fix("start Docker: `sudo dockerd &` or `sudo systemctl start docker`")
            .fix("confirm with `docker version`")
            .into_error());
    }
    Ok(docker)
}

/// The measurement policy for a problem, from its declared limits.
pub fn policy_for(problem: &Problem) -> SandboxPolicy {
    let limits = &problem.meta.limits;
    let mut policy = SandboxPolicy::default();
    policy.timeout_ms = limits.time_ms;
    policy.memory_bytes = limits.memory_bytes;
    policy.max_output_bytes = limits.output_bytes;
    // The container needs room for the runtime on top of the child's cap.
    policy.container_memory_bytes = limits.memory_bytes + limits.memory_bytes / 2;
    policy
}

/// Host facts, gathered once per command.
pub fn host_info() -> Host {
    ccx_core::host::detect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(language: &str, toolchain: &str) -> SolutionConfig {
        SolutionConfig {
            problem: "problem".into(),
            language: language.into(),
            toolchain: toolchain.into(),
            opt_level: "-O2".into(),
            build_preset: BuildPreset::Standard,
            compiler_version: None,
            compiler_flags: Vec::new(),
            program_args: Vec::new(),
            build_command: Vec::new(),
            run_command: Vec::new(),
            target_os: "linux".into(),
            target_arch: "x86_64".into(),
            build_system: BuildSystem::Direct,
            cmake_generator: CmakeGenerator::default(),
            cmake_target: default_cmake_target(),
        }
    }

    #[test]
    fn standard_configuration_rejects_custom_flags() {
        assert!(validate_build_customization(
            Language::Cpp,
            BuildPreset::Standard,
            BuildSystem::Direct,
            &["-mavx2".into()]
        )
        .is_err());
    }

    #[test]
    fn custom_configuration_accepts_safe_codegen_flags() {
        assert!(validate_build_customization(
            Language::Cpp,
            BuildPreset::Custom,
            BuildSystem::Direct,
            &[
                "-mavx2".into(),
                "-funroll-loops".into(),
                "-std=c++23".into()
            ]
        )
        .is_ok());
    }

    #[test]
    fn custom_configuration_rejects_unsafe_or_host_specific_flags() {
        for flag in [
            "-fplugin=evil.so",
            "@flags.rsp",
            "-march=native",
            "-o",
            "-DNAME=$(id)",
        ] {
            assert!(
                validate_build_customization(
                    Language::Cpp,
                    BuildPreset::Custom,
                    BuildSystem::Direct,
                    &[flag.into()]
                )
                .is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn custom_compiler_templates_keep_outputs_and_sources_fixed() {
        let default = default_build_command();
        let reordered = ["{compiler}", "{sources}", "{flags}", "-o", "{output}"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(validate_build_command(Language::Cpp, BuildSystem::Custom, &default).is_ok());
        assert!(validate_build_command(Language::C, BuildSystem::Custom, &reordered).is_ok());
        assert!(validate_build_command(Language::Rust, BuildSystem::Custom, &default).is_err());
        assert!(validate_build_command(
            Language::Cpp,
            BuildSystem::Custom,
            &["sh".into(), "-c".into(), "g++ ...".into()]
        )
        .is_err());
        assert!(validate_build_command(Language::Cpp, BuildSystem::Direct, &default).is_err());
    }

    #[test]
    fn runtime_command_templates_are_bounded_and_explicit() {
        let command = ["env", "CCX_MODE=fast", "{binary}", "--input", "{args}"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert!(validate_run_command(&command).is_ok());
        assert!(validate_run_command(&[]).is_ok());
        assert!(validate_run_command(&["{binary}".into()]).is_err());
        assert!(validate_run_command(&["{args}".into(), "{binary}".into()]).is_err());
        assert!(
            validate_run_command(&["{binary}".into(), "{args}".into(), "{unknown}".into()])
                .is_err()
        );
    }

    #[test]
    fn target_defaults_to_the_locked_linux_profile() {
        let parsed: SolutionConfig =
            toml::from_str("problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\n")
                .unwrap();
        assert_eq!(parsed.target_os, "linux");
        assert_eq!(parsed.target_arch, "x86_64");
        assert_eq!(parsed.build_preset, BuildPreset::Standard);
        assert!(parsed.program_args.is_empty());
    }

    #[test]
    fn versioned_gcc_and_clang_profiles_are_selectable_for_cpp() {
        let language = config("cpp", "gcc-13").language().unwrap();
        assert_eq!(
            config("cpp", "gcc-13").toolchain(language).unwrap(),
            ToolchainName::Gcc13
        );
        assert_eq!(
            config("cpp", "clang-18").toolchain(language).unwrap(),
            ToolchainName::Clang18
        );
        assert!(config("d", "gcc-13")
            .toolchain(config("d", "auto").language().unwrap())
            .is_err());
    }

    #[test]
    fn runtime_arguments_are_preserved_and_bounded() {
        let parsed: SolutionConfig = toml::from_str(
            "problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\nprogram_args = [\"--mode\", \"fast path\"]\n",
        )
        .unwrap();
        assert_eq!(parsed.program_args, ["--mode", "fast path"]);
        assert!(validate_program_args(&parsed.program_args).is_ok());
        assert!(validate_program_args(&vec!["x".repeat(4097)]).is_err());
        assert!(validate_program_args(&vec!["x".repeat(4096); 5]).is_err());
        assert!(validate_program_args(&vec!["\0".to_string()]).is_err());
    }

    #[test]
    fn assembly_is_not_a_selectable_solution_language() {
        let assembly = SolutionConfig {
            language: "asm".into(),
            ..config("cpp", "gcc")
        };
        assert!(assembly.language().is_err());
    }

    #[test]
    fn cmake_generator_defaults_to_ninja_and_accepts_makefiles() {
        let default: SolutionConfig =
            toml::from_str("problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\n")
                .unwrap();
        let makefiles: SolutionConfig = toml::from_str(
            "problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\ncmake_generator = \"unix_makefiles\"\n",
        )
        .unwrap();
        assert_eq!(default.cmake_generator, CmakeGenerator::Ninja);
        assert_eq!(makefiles.cmake_generator, CmakeGenerator::UnixMakefiles);
    }

    #[test]
    fn cmake_target_defaults_and_accepts_safe_executable_names() {
        let default: SolutionConfig =
            toml::from_str("problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\n")
                .unwrap();
        let custom: SolutionConfig = toml::from_str(
            "problem = \"problem\"\nlanguage = \"cpp\"\ntoolchain = \"gcc\"\nbuild_system = \"cmake\"\ncmake_target = \"solver-v2+fast\"\n",
        )
        .unwrap();
        assert_eq!(default.cmake_target, "ccx_solution");
        assert_eq!(custom.cmake_target, "solver-v2+fast");
        assert!(validate_cmake_target(&custom.cmake_target).is_ok());
    }

    #[test]
    fn cmake_target_rejects_shell_path_and_builtin_names() {
        for target in [
            "",
            "-solver",
            "../solver",
            "solver;touch",
            "solver $(id)",
            "all",
            "clean",
            "test",
        ] {
            assert!(validate_cmake_target(target).is_err(), "{target:?}");
        }
        assert!(validate_cmake_target(&"x".repeat(129)).is_err());
    }

    #[test]
    fn generated_matrix_capsules_are_not_solution_inputs() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ccx-source-collection-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("matrix")).unwrap();
        std::fs::write(root.join("main.cpp"), "int main() { return 0; }\n").unwrap();
        std::fs::write(root.join("matrix/gcc-o2.capsule.json"), "{}\n").unwrap();

        let mut sources = std::collections::BTreeMap::new();
        let mut project_files = std::collections::BTreeMap::new();
        collect_project_files(&root, &root, "cpp", &mut sources, &mut project_files).unwrap();

        assert!(sources.contains_key("main.cpp"));
        assert!(!project_files.contains_key("matrix/gcc-o2.capsule.json"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn d_selects_gdc_automatically() {
        let config = config("d", "auto");
        let language = config.language().unwrap();
        assert_eq!(config.toolchain(language).unwrap(), ToolchainName::Gdc);
    }

    #[test]
    fn d_rejects_a_non_gdc_toolchain() {
        let config = config("d", "gcc");
        let language = config.language().unwrap();
        assert!(config.toolchain(language).is_err());
    }

    #[test]
    fn zig_selects_its_pinned_toolchain() {
        let cfg = config("zig", "auto");
        let language = cfg.language().unwrap();
        assert_eq!(cfg.toolchain(language).unwrap(), ToolchainName::Zig);
        assert!(config("zig", "gcc").toolchain(language).is_err());
    }

    #[test]
    fn odin_selects_its_pinned_toolchain() {
        let cfg = config("odin", "auto");
        let language = cfg.language().unwrap();
        assert_eq!(cfg.toolchain(language).unwrap(), ToolchainName::Odin);
        assert!(config("odin", "gcc").toolchain(language).is_err());
    }

    #[test]
    fn go_selects_its_pinned_toolchain() {
        let cfg = config("go", "auto");
        let language = cfg.language().unwrap();
        assert_eq!(cfg.toolchain(language).unwrap(), ToolchainName::Go);
        assert!(config("go", "gcc").toolchain(language).is_err());
    }
}
