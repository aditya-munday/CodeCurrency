//! Shared command context: locating a solution, loading its problem, and
//! talking to Docker.
//!
//! Every command needs the same three things — the solution directory, the
//! problem it targets, and the pinned toolchain — so they are resolved once
//! here. This is also the single place that decides what a "solution folder"
//! looks like on disk.

use crate::cmd::Fixable;
use ccx_core::capsule::{Language, ToolchainName};
use ccx_core::capsule::Host;
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
    pub language: Language,
    pub toolchain_name: ToolchainName,
    pub opt_level: String,
}

impl Solution {
    /// Discover and load the solution rooted at `dir`.
    pub fn load(dir: &Path) -> anyhow::Result<Solution> {
        let root = dir
            .canonicalize()
            .map_err(|e| {
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
            Fixable::new(format!("problem at {} failed to load: {e}", problem_root.display()))
                .fix("check the problem folder has problem.json, generators, and solutions")
                .fix("run `ccx validate <problem-folder>` for a full report")
                .into_error()
        })?;

        let language = cfg.language();
        let ext = toolchain::extension(language);
        let mut sources = std::collections::BTreeMap::new();

        // Every source file in the folder is part of the solution's identity.
        // A hidden directory (.ccx, .git) is build state, not source.
        for entry in std::fs::read_dir(&root)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) == Some(ext) {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();
                sources.insert(name, std::fs::read_to_string(&path)?);
            }
        }

        if sources.is_empty() {
            return Err(Fixable::new(format!(
                "no .{ext} source files found in {}",
                root.display()
            ))
            .fix(format!("add a .{ext} file, or run `ccx init` to scaffold one"))
            .into_error());
        }

        Ok(Solution {
            root,
            problem,
            sources,
            language,
            toolchain_name: cfg.toolchain(),
            opt_level: cfg.opt_level,
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
        Ok(Solution {
            root: problem.root.clone(),
            problem: problem.clone(),
            sources,
            language: Language::Cpp,
            toolchain_name: ToolchainName::Gcc,
            opt_level: "-O2".to_string(),
        })
    }

    pub fn ensure_dirs(&self) -> anyhow::Result<()> {
        std::fs::create_dir_all(self.out_dir())?;
        Ok(())
    }
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
}

fn default_language() -> String {
    "cpp".into()
}

fn default_toolchain() -> String {
    "gcc".into()
}

fn default_opt() -> String {
    "-O2".into()
}

impl SolutionConfig {
    pub fn language(&self) -> Language {
        match self.language.as_str() {
            "c" => Language::C,
            "asm" => Language::AsmX86_64,
            _ => Language::Cpp,
        }
    }

    pub fn toolchain(&self) -> ToolchainName {
        match self.toolchain.as_str() {
            "clang" => ToolchainName::Clang,
            _ => ToolchainName::Gcc,
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
