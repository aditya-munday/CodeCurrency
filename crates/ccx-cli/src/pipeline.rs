//! Shared pipeline steps: build artifacts, generate input, run programs.
//!
//! `test` and `measure` are both sequences of these steps, so they live here
//! once rather than being reimplemented per command. Every step keeps the same
//! boundary discipline as `build`: read-only mounts, tmpfs scratch, artifacts
//! copied out, and untrusted programs only ever run under the runner.

use crate::cmd::Fixable;
use crate::context::{policy_for, BuildSystem, CmakeGenerator, Solution};
use ccx_core::engine::{parse_framed_report, Docker, RunOutcome};
use ccx_core::measure::parse_cachegrind_summary;
use ccx_core::sandbox::SandboxPolicy;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static INPUT_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);
static PIPELINE_DIRECTORY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Source-file extension for a language, for callers that only need the name.
pub fn extension_for(language: ccx_core::capsule::Language) -> &'static str {
    ccx_core::toolchain::extension(language)
}

fn zig_target_flags() -> Vec<String> {
    vec![
        "-target".to_string(),
        "x86_64-linux-gnu".to_string(),
        "-mcpu=baseline".to_string(),
    ]
}

/// Guest path where a compiled program's directory is mounted read-only.
const GUEST_PROG: &str = "/prog";
/// Guest path where test input is mounted read-only.
const GUEST_INPUT: &str = "/input";
/// Guest path the runner reads as the program's stdin.
const GUEST_STDIN: &str = "/input/stdin.txt";
/// Guest path for the allocation profile.
const GUEST_ALLOC: &str = "/work/scratch/alloc-%p.xtree";
const CACHEGRIND_ARGS: &[&str] = &[
    "--tool=cachegrind",
    "--cache-sim=yes",
    "--branch-sim=yes",
    "--I1=32768,8,64",
    "--D1=32768,8,64",
    "--trace-children=yes",
    "--cachegrind-out-file=/dev/null",
];

/// A compiled program and how to invoke it inside the sandbox.
#[derive(Debug, Clone)]
pub struct Program {
    /// Host directory holding the binary, mounted read-only at `/prog`.
    pub host_dir: PathBuf,
    /// Guest path of the binary itself.
    pub guest_binary: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    pub runtime_argv: Option<Vec<String>>,
    pub runtime_prefix: Vec<String>,
    /// Command prefix placed before the binary, e.g. `valgrind`. Empty for a
    /// plain run.
    pub prefix: Vec<String>,
}

impl Program {
    fn command_argv(&self) -> Vec<String> {
        let mut command = self.runtime_prefix.clone();
        if let Some(runtime_argv) = &self.runtime_argv {
            command.extend(runtime_argv.iter().cloned());
        } else {
            command.push(self.guest_binary.clone());
            command.extend(self.args.iter().cloned());
        }
        command
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildTiming {
    pub wall_ns: u64,
    pub user_cpu_ns: Option<u64>,
    pub system_cpu_ns: Option<u64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OptimizationRemarks {
    pub status: String,
    pub method: Option<String>,
    pub explanation: Option<String>,
    pub text: String,
}

fn filter_optimization_remarks(stderr: &str, source_filters: &[String]) -> String {
    stderr
        .lines()
        .filter(|line| {
            source_filters
                .iter()
                .any(|source_filter| line.contains(source_filter))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub struct BatchExecution {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub struct Pipeline<'a> {
    pub sol: &'a Solution,
    pub docker: Docker,
    pub image: String,
    work_dir: PathBuf,
    source_dir: PathBuf,
}

fn stage_project_files(sol: &Solution, source_dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir(source_dir)?;
    let mut directories = std::collections::BTreeSet::from([source_dir.to_path_buf()]);
    for (relative, contents) in &sol.project_files {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            return Err(anyhow::anyhow!("unsafe project path {relative:?}"));
        }
        let destination = source_dir.join(relative_path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
            let mut directory = source_dir.to_path_buf();
            for component in parent.strip_prefix(source_dir)?.components() {
                directory.push(component);
                directories.insert(directory.clone());
            }
        }
        std::fs::write(&destination, contents)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o444))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for directory in directories {
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    Ok(())
}

impl<'a> Pipeline<'a> {
    pub fn new(sol: &'a Solution) -> anyhow::Result<Self> {
        let docker = crate::context::require_docker()?;
        let tc = sol.toolchain()?;
        let base_dir = sol.work_dir();
        std::fs::create_dir_all(&base_dir)?;
        let work_dir = loop {
            let sequence = PIPELINE_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = base_dir.join(format!("run-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        };
        let source_dir = work_dir.join("source");
        stage_project_files(sol, &source_dir)?;
        Ok(Pipeline {
            sol,
            docker,
            image: Docker::image_ref(&tc),
            work_dir,
            source_dir,
        })
    }

    fn scratch_dir(&self) -> PathBuf {
        self.work_dir.clone()
    }

    fn assembly_output_path(&self) -> PathBuf {
        let run_name = self
            .work_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("run");
        self.sol.out_dir().join(format!("{run_name}-out.s"))
    }

    fn create_input_dir(&self) -> anyhow::Result<PathBuf> {
        loop {
            let sequence = INPUT_DIRECTORY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = self
                .scratch_dir()
                .join(format!("input-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn expand_runtime_argv(&self, guest_binary: &str, args: &[String]) -> Option<Vec<String>> {
        if self.sol.run_command.is_empty() {
            return None;
        }
        let mut argv = Vec::new();
        for token in &self.sol.run_command {
            match token.as_str() {
                "{binary}" => argv.push(guest_binary.to_string()),
                "{args}" => argv.extend(args.iter().cloned()),
                value => argv.push(value.to_string()),
            }
        }
        Some(argv)
    }

    pub fn expected_runtime_argv(&self) -> Vec<String> {
        self.expand_runtime_argv("/prog/sol", &self.sol.program_args)
            .unwrap_or_default()
    }

    fn copy_batch_program(
        &self,
        program: &Program,
        destination: &Path,
        name: &str,
    ) -> anyhow::Result<()> {
        let source_name = Path::new(&program.guest_binary)
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("batch program has no binary name"))?;
        let source = program.host_dir.join(source_name);
        let target = destination.join(name);
        std::fs::copy(source, &target)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o555))?;
        }
        Ok(())
    }

    pub fn run_batch(
        &self,
        generator: &Program,
        reference: &Program,
        solution: &Program,
        checker: Option<&Program>,
        policy: &SandboxPolicy,
        size: &str,
        first_seed: u64,
        count: u64,
    ) -> anyhow::Result<BatchExecution> {
        if count == 0 || count > 10_000 {
            return Err(anyhow::anyhow!(
                "batch size must be between 1 and 10000 cases"
            ));
        }
        let scale = *self.sol.problem.meta.sizes.get(size).ok_or_else(|| {
            Fixable::new(format!("problem has no size named {size:?}"))
                .fix("select a size listed in problem.json")
                .into_error()
        })?;
        let batch_dir = self.create_input_dir()?;
        let result = (|| {
            self.copy_batch_program(generator, &batch_dir, "gen")?;
            self.copy_batch_program(reference, &batch_dir, "ref")?;
            self.copy_batch_program(solution, &batch_dir, "sol")?;
            if let Some(checker) = checker {
                self.copy_batch_program(checker, &batch_dir, "checker")?;
            }

            let solution_memory = if self.sol.language == ccx_core::capsule::Language::Go {
                8 * 1024 * 1024 * 1024u64
            } else {
                policy.memory_bytes
            };
            let mut command = vec![
                "/opt/ccx/batch-runner".to_string(),
                "--first-seed".to_string(),
                first_seed.to_string(),
                "--count".to_string(),
                count.to_string(),
                "--scale".to_string(),
                scale.to_string(),
                "--timeout-ms".to_string(),
                policy.timeout_ms.to_string(),
                "--memory-bytes".to_string(),
                policy.memory_bytes.to_string(),
                "--solution-memory-bytes".to_string(),
                solution_memory.to_string(),
                "--stack-bytes".to_string(),
                "8388608".to_string(),
                "--pids-limit".to_string(),
                policy.pids_limit.to_string(),
                "--max-output-bytes".to_string(),
                policy.max_output_bytes.to_string(),
                "--generator".to_string(),
                "/prog/gen".to_string(),
                "--reference".to_string(),
                "/prog/ref".to_string(),
                "--checker".to_string(),
                if checker.is_some() {
                    "/prog/checker".to_string()
                } else {
                    "-".to_string()
                },
            ];
            let solution_command = solution.command_argv();
            command.push("--solution-command-count".to_string());
            command.push(solution_command.len().to_string());
            command.extend(solution_command);

            let mut run_policy = policy.clone();
            run_policy.container_memory_bytes = run_policy
                .container_memory_bytes
                .saturating_add(256 * 1024 * 1024);
            let argv = run_policy.argv_mounted(
                &self.image,
                &command,
                &[(
                    batch_dir.to_string_lossy().to_string(),
                    GUEST_PROG.to_string(),
                )],
            );
            let (status, stdout, stderr) = self.docker.run(&argv)?;
            Ok(BatchExecution {
                status,
                stdout,
                stderr,
            })
        })();
        let _ = std::fs::remove_dir_all(batch_dir);
        result
    }

    fn c_driver(&self) -> &'static str {
        match self.sol.toolchain_name {
            ccx_core::capsule::ToolchainName::Gcc13 => "gcc-13",
            ccx_core::capsule::ToolchainName::Clang18 => "clang-18",
            ccx_core::capsule::ToolchainName::Clang | ccx_core::capsule::ToolchainName::Odin => {
                "clang"
            }
            _ => "gcc",
        }
    }

    fn cpp_driver(&self) -> &'static str {
        match self.sol.toolchain_name {
            ccx_core::capsule::ToolchainName::Gcc13 => "g++-13",
            ccx_core::capsule::ToolchainName::Clang18 => "clang++-18",
            ccx_core::capsule::ToolchainName::Clang | ccx_core::capsule::ToolchainName::Odin => {
                "clang++"
            }
            _ => "g++",
        }
    }

    fn cmake_script(&self) -> String {
        let compiler = if self.sol.language == ccx_core::capsule::Language::C {
            self.c_driver()
        } else {
            self.cpp_driver()
        };
        let (language, flags_key, standard) = if self.sol.language == ccx_core::capsule::Language::C
        {
            ("C", "C", "17")
        } else {
            ("CXX", "CXX", "20")
        };
        let compile_flags = format!(
            "{} {} -ffile-prefix-map=/src0=. -fno-ident",
            self.sol.opt_level,
            self.sol.compiler_flags.join(" ")
        );
        let link_flags = "-Wl,--build-id=none".to_string();
        let generator = match self.sol.cmake_generator {
            CmakeGenerator::Ninja => "Ninja",
            CmakeGenerator::UnixMakefiles => "Unix Makefiles",
        };
        format!(
            "cmake -S /src0 -B /work/scratch/cmake -G '{generator}' -DCMAKE_BUILD_TYPE= -DCMAKE_RUNTIME_OUTPUT_DIRECTORY=/work/scratch -DCMAKE_CXX_SCAN_FOR_MODULES=OFF -DCMAKE_{language}_COMPILER={compiler} -DCMAKE_{flags_key}_STANDARD={standard} -DCMAKE_{flags_key}_STANDARD_REQUIRED=ON -DCMAKE_{flags_key}_FLAGS='{compile_flags}' -DCMAKE_EXE_LINKER_FLAGS='{link_flags}' && cmake --build /work/scratch/cmake --target ccx_solution --parallel 1 && cp /work/scratch/ccx_solution /work/scratch/sol"
        )
    }

    /// Compile one artifact and carry the binary out of the sandbox.
    ///
    /// The compiler runs as the runner's child, so the compiler is itself
    /// contained: a hostile source file cannot reach the host through the
    /// compiler. The binary is then emitted as a length-prefixed frame on the
    /// runner's own stdout — the same framing the measurement report uses.
    ///
    /// This is why no writable host mount and no named container are needed: the
    /// source mount stays read-only, scratch stays tmpfs, and the one artifact
    /// that crosses the boundary is framed and length-checked on the way out.
    fn build_artifact(
        &self,
        driver: &str,
        std_flags: &[String],
        sources: &[PathBuf],
        host_dir: &Path,
        out_name: &str,
        with_shim: bool,
    ) -> anyhow::Result<(PathBuf, BuildTiming, Vec<String>)> {
        std::fs::create_dir_all(host_dir)?;

        let custom = driver == "ccx-custom";
        let mut compiler: Vec<String> = if custom {
            Vec::new()
        } else if driver == "ccx-cmake" {
            vec![
                "sh".to_string(),
                "-eu".to_string(),
                "-c".to_string(),
                self.cmake_script(),
            ]
        } else if driver == "go" {
            vec![
                "env".to_string(),
                "GO111MODULE=off".to_string(),
                "GOTOOLCHAIN=local".to_string(),
                "GOPROXY=off".to_string(),
                "GOSUMDB=off".to_string(),
                "GOENV=off".to_string(),
                "GOROOT=/usr/local/go".to_string(),
                "GOCACHE=/work/scratch/go-cache".to_string(),
                "GOTMPDIR=/work/scratch".to_string(),
                "go".to_string(),
            ]
        } else {
            vec![driver.to_string()]
        };
        compiler.extend(std_flags.iter().cloned());
        if !custom && driver == "go" {
            compiler.extend(["-o".to_string(), format!("/work/scratch/{out_name}")]);
        } else if !custom && driver == "zig" {
            if out_name == "out.s" {
                compiler.push("-femit-asm=/work/scratch/out.s".to_string());
            } else {
                compiler.push(format!("-femit-bin=/work/scratch/{out_name}"));
            }
            compiler.extend([
                "--cache-dir".to_string(),
                "/work/scratch/zig-cache".to_string(),
                "--global-cache-dir".to_string(),
                "/work/scratch/zig-global-cache".to_string(),
            ]);
        } else if !custom && driver == "odin" {
        } else if !custom && driver == "rustc" {
            compiler.push("--remap-path-prefix=/src0=.".to_string());
        } else if !custom && driver != "ccx-cmake" {
            compiler.extend([
                "-ffile-prefix-map=/in=.".to_string(),
                "-fno-ident".to_string(),
                "-Wl,--build-id=none".to_string(),
            ]);
        }
        if !custom && !matches!(driver, "zig" | "odin" | "go" | "ccx-cmake") {
            compiler.extend(["-o".to_string(), format!("/work/scratch/{out_name}")]);
        }

        // Mount each distinct source directory read-only at its own mount point,
        // so two files with the same name in different folders cannot collide.
        let mut mounts: Vec<(String, String)> = Vec::new();
        if custom {
            mounts.push((
                self.source_dir.to_string_lossy().to_string(),
                "/src0".to_string(),
            ));
        } else if !sources.is_empty() && sources.iter().all(|src| src.starts_with(&self.source_dir))
        {
            mounts.push((
                self.source_dir.to_string_lossy().to_string(),
                "/src0".to_string(),
            ));
            for src in sources {
                let relative = src.strip_prefix(&self.source_dir)?;
                if driver != "ccx-cmake" {
                    let guest_source = if driver == "odin" && relative.as_os_str().is_empty() {
                        "/src0/.".to_string()
                    } else {
                        format!("/src0/{}", relative.to_string_lossy())
                    };
                    compiler.push(guest_source);
                }
            }
        } else {
            // Mount each distinct source directory read-only at its own mount point,
            // so two files with the same name in different folders cannot collide.
            for src in sources {
                let parent = src.parent().unwrap_or(Path::new(".")).to_path_buf();
                let name = src
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| anyhow::anyhow!("bad source path {}", src.display()))?;
                let host = parent.to_string_lossy().to_string();
                let guest = match mounts.iter().find(|(h, _)| h == &host) {
                    Some((_, guest)) => guest.clone(),
                    None => {
                        let guest = format!("/src{}", mounts.len());
                        mounts.push((host, guest.clone()));
                        guest
                    }
                };
                if driver != "ccx-cmake" {
                    compiler.push(format!("{guest}/{name}"));
                }
            }
        }
        if with_shim {
            // `--wrap` only rewrites calls in objects we link, so allocations
            // made inside a *shared* libstdc++ are invisible to the shim and
            // `std::vector` would report zero. Linking libstdc++/libgcc
            // statically puts those call sites in our link, which makes the
            // count real. This is a measurement requirement, not a
            // performance one: the reported number is wrong without it.
            if driver == "rustc" {
                compiler.extend([
                    "-Clink-arg=-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free,--wrap=posix_memalign,--wrap=aligned_alloc,--wrap=memalign"
                        .to_string(),
                    "-Clink-arg=/opt/ccx/alloc_shim.o".to_string(),
                ]);
            } else {
                compiler.extend([
                    "-static-libstdc++".to_string(),
                    "-static-libgcc".to_string(),
                    "-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free,--wrap=posix_memalign,--wrap=aligned_alloc,--wrap=memalign".to_string(),
                    "/opt/ccx/alloc_shim.o".to_string(),
                ]);
            }
        }

        // The compiler is the runner's child, so it needs room to run: a large
        // translation unit is not a hostile infinite loop, but it is not fast
        // either. The wall clock is generous; the memory cap stays the default.
        let mut policy = SandboxPolicy::default();
        policy.timeout_ms = 300_000;
        policy.memory_bytes = 2 * 1024 * 1024 * 1024;
        policy.container_memory_bytes = 3 * 1024 * 1024 * 1024;
        if driver == "go" {
            policy.scratch_bytes = 2 * 1024 * 1024 * 1024;
            policy.scratch_tmpfs = "2g".to_string();
        }
        policy.cpuset = None;

        let mut command: Vec<String> = vec!["/opt/ccx/runner".to_string()];
        command.extend([
            "--timeout-ms".to_string(),
            policy.timeout_ms.to_string(),
            "--memory-bytes".to_string(),
            policy.memory_bytes.to_string(),
            "--stack-bytes".to_string(),
            "8388608".to_string(),
            "--pids-limit".to_string(),
            policy.pids_limit.to_string(),
            "--max-output-bytes".to_string(),
            (8 * 1024 * 1024u64).to_string(),
            "--cwd".to_string(),
            "/work/scratch".to_string(),
            "--emit-file".to_string(),
            format!("/work/scratch/{out_name}"),
            "--".to_string(),
        ]);
        command.extend(compiler.iter().cloned());

        let argv = policy.argv_mounted(&self.image, &command, &mounts);
        // Raw bytes, not text: the emitted frame holds a binary, and decoding it
        // as UTF-8 would corrupt the artifact and break the length prefix.
        let (_, stdout, stderr) = self.docker.run_bytes(&argv)?;
        let stderr = String::from_utf8_lossy(&stderr).to_string();
        let (outcome, emitted) = ccx_core::engine::parse_framed_report_with_emit(&stdout)
            .ok_or_else(|| {
                Fixable::new(format!(
                    "the compiler produced no usable report\n\nstderr:\n{stderr}"
                ))
                .fix("this is an engine bug: report it with the problem id")
                .into_error()
            })?;

        if outcome.artifact_too_large {
            return Err(
                Fixable::new("the compiled artifact exceeds the 64 MiB transfer limit")
                    .fix("reduce the binary size before running this solution in the arena")
                    .into_error(),
            );
        }

        if outcome.exit_status != 0 {
            let diag = if outcome.stderr.trim().is_empty() {
                outcome.stdout.clone()
            } else {
                outcome.stderr.clone()
            };
            return Err(Fixable::new(format!(
                "compiling {out_name} failed (exit {})\n\n{diag}",
                outcome.exit_status
            ))
            .fix("fix the compiler errors above")
            .fix(format!("the exact command was:\n  {}", compiler.join(" ")))
            .into_error());
        }

        let Some(binary) = emitted else {
            return Err(Fixable::new(format!(
                "the compiler reported success but produced no {out_name} binary"
            ))
            .fix("this is an engine bug: report it with the command above")
            .into_error());
        };

        let host_artifact = host_dir.join(out_name);
        std::fs::write(&host_artifact, &binary)?;
        // A compiler that succeeds without producing a runnable file is a bug in
        // the build, not a property of the submission.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&host_artifact)?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&host_artifact, perms)?;
        }
        #[cfg(not(unix))]
        {
            let perms = std::fs::metadata(&host_artifact)?.permissions();
            std::fs::set_permissions(&host_artifact, perms)?;
        }
        Ok((
            host_artifact,
            BuildTiming {
                wall_ns: outcome.wall_ns,
                user_cpu_ns: outcome
                    .resources
                    .as_ref()
                    .and_then(|resources| resources.user_cpu_ns),
                system_cpu_ns: outcome
                    .resources
                    .as_ref()
                    .and_then(|resources| resources.system_cpu_ns),
            },
            compiler,
        ))
    }

    /// Compile the problem's input generator (`generator.c`).
    ///
    /// Returns a `Program` whose guest path is where the run-time `/prog` mount
    /// exposes the binary — not where the build wrote it.
    pub fn build_generator(&self) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join("gen");
        let _ = self.build_artifact(
            self.c_driver(),
            &["-O2".to_string(), "-std=c17".to_string()],
            &[self.sol.problem.root.join("generator.c")],
            &dir,
            "gen",
            false,
        )?;
        Ok(Program {
            host_dir: dir,
            guest_binary: format!("{GUEST_PROG}/gen"),
            args: vec![],
            runtime_argv: None,
            runtime_prefix: vec![],
            prefix: vec![],
        })
    }

    /// Compile the problem's reference solution (`reference.cpp`).
    pub fn build_reference(&self) -> anyhow::Result<Program> {
        self.build_reference_with_argv().map(|(program, _)| program)
    }

    pub fn build_reference_with_argv(&self) -> anyhow::Result<(Program, Vec<String>)> {
        let dir = self.scratch_dir().join("ref");
        let (_, _, argv) = self.build_artifact(
            self.cpp_driver(),
            &["-O2".to_string(), "-std=c++20".to_string()],
            &[self.sol.problem.root.join("reference.cpp")],
            &dir,
            "ref",
            false,
        )?;
        Ok((
            Program {
                host_dir: dir,
                guest_binary: format!("{GUEST_PROG}/ref"),
                args: vec![],
                runtime_argv: None,
                runtime_prefix: vec![],
                prefix: vec![],
            },
            argv,
        ))
    }

    pub fn build_problem_cpp(&self, source: &str, name: &str) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join(name);
        let _ = self.build_artifact(
            self.cpp_driver(),
            &["-O2".to_string(), "-std=c++20".to_string()],
            &[self.sol.problem.root.join(source)],
            &dir,
            name,
            true,
        )?;
        Ok(Program {
            host_dir: dir,
            guest_binary: format!("{GUEST_PROG}/{name}"),
            args: vec![],
            runtime_argv: None,
            runtime_prefix: vec![],
            prefix: vec![],
        })
    }

    pub fn build_checker(&self) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join("checker");
        let _ = self.build_artifact(
            self.c_driver(),
            &["-O2".to_string(), "-std=c17".to_string()],
            &[self.sol.problem.root.join("checker.c")],
            &dir,
            "checker",
            false,
        )?;
        Ok(Program {
            host_dir: dir,
            guest_binary: format!("{GUEST_PROG}/checker"),
            args: vec![],
            runtime_argv: None,
            runtime_prefix: vec![],
            prefix: vec![],
        })
    }

    pub fn check_output(
        &self,
        checker: &Program,
        input: &[u8],
        expected: &[u8],
        actual: &[u8],
        policy: &SandboxPolicy,
    ) -> anyhow::Result<bool> {
        let input_dir = self.create_input_dir()?;
        std::fs::write(input_dir.join("expected.txt"), expected)?;
        std::fs::write(input_dir.join("actual.txt"), actual)?;
        let mut invocation = checker.clone();
        invocation.args = vec![
            format!("{GUEST_STDIN}"),
            format!("{GUEST_INPUT}/expected.txt"),
            format!("{GUEST_INPUT}/actual.txt"),
        ];
        let outcome =
            self.run_program_in_dir(&invocation, input, policy, false, false, false, &input_dir);
        let _ = std::fs::remove_dir_all(&input_dir);
        let outcome = outcome?;
        Ok(outcome.verdict(policy.memory_bytes) == ccx_core::capsule::Verdict::Accepted)
    }

    /// Compile the user's solution. The binary is returned as a `Program`.
    pub fn build_solution(&self) -> anyhow::Result<Program> {
        self.build_solution_timed().map(|(program, _)| program)
    }

    pub fn build_solution_timed(&self) -> anyhow::Result<(Program, BuildTiming)> {
        let dir = self.scratch_dir().join("sol");
        if self.sol.build_system == BuildSystem::Custom {
            let argv = self.custom_build_argv();
            let (_, timing, _) =
                self.build_artifact("ccx-custom", &argv, &[], &dir, "sol", false)?;
            let args = self.sol.program_args.clone();
            return Ok((
                Program {
                    host_dir: dir,
                    guest_binary: format!("{GUEST_PROG}/sol"),
                    runtime_argv: self.expand_runtime_argv("/prog/sol", &args),
                    runtime_prefix: vec![],
                    args,
                    prefix: vec![],
                },
                timing,
            ));
        }
        if self.sol.build_system == BuildSystem::Cmake {
            let (_, timing, _) = self.build_artifact(
                "ccx-cmake",
                &[],
                &[self.source_dir.join("CMakeLists.txt")],
                &dir,
                "sol",
                false,
            )?;
            return Ok((
                Program {
                    host_dir: dir,
                    guest_binary: format!("{GUEST_PROG}/sol"),
                    runtime_argv: self.expand_runtime_argv("/prog/sol", &self.sol.program_args),
                    runtime_prefix: vec![],
                    args: self.sol.program_args.clone(),
                    prefix: vec![],
                },
                timing,
            ));
        }
        let driver = ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let (flags, sources, with_shim) = match self.sol.language {
            ccx_core::capsule::Language::Rust => {
                let entry = if self.source_dir.join("main.rs").is_file() {
                    self.source_dir.join("main.rs")
                } else {
                    self.source_dir
                        .join(self.sol.sources.keys().next().unwrap())
                };
                let opt = self.sol.opt_level.trim_start_matches("-O");
                (
                    vec!["--edition=2021".to_string(), format!("-Copt-level={opt}")],
                    vec![entry],
                    false,
                )
            }
            ccx_core::capsule::Language::D => (
                vec![self.sol.opt_level.clone()],
                self.sol
                    .sources
                    .keys()
                    .map(|name| self.source_dir.join(name))
                    .collect(),
                false,
            ),
            ccx_core::capsule::Language::Zig => {
                let mode = match self.sol.opt_level.as_str() {
                    "-O0" => "Debug",
                    "-Os" | "-Oz" => "ReleaseSmall",
                    _ => "ReleaseFast",
                };
                let mut flags = vec![
                    "build-exe".to_string(),
                    format!("-O{mode}"),
                    "-lc".to_string(),
                ];
                flags.extend(zig_target_flags());
                (
                    flags,
                    vec![if self.source_dir.join("main.zig").is_file() {
                        self.source_dir.join("main.zig")
                    } else {
                        self.source_dir
                            .join(self.sol.sources.keys().next().unwrap())
                    }],
                    false,
                )
            }
            ccx_core::capsule::Language::Odin => {
                let optimization = if self.sol.opt_level == "-O0" {
                    "-debug"
                } else {
                    "-o:speed"
                };
                (
                    vec![
                        "build".to_string(),
                        optimization.to_string(),
                        "-out:/work/scratch/sol".to_string(),
                    ],
                    vec![self.source_dir.clone()],
                    false,
                )
            }
            ccx_core::capsule::Language::Go => {
                let mut flags = vec![
                    "build".to_string(),
                    "-trimpath".to_string(),
                    "-buildvcs=false".to_string(),
                    "-ldflags=-buildid=".to_string(),
                ];
                if self.sol.opt_level == "-O0" {
                    flags.push("-gcflags=all=-N".to_string());
                }
                (
                    flags,
                    self.sol
                        .sources
                        .keys()
                        .map(|name| self.source_dir.join(name))
                        .collect(),
                    false,
                )
            }
            ccx_core::capsule::Language::AsmX86_64 => (
                Vec::new(),
                self.sol
                    .sources
                    .keys()
                    .map(|name| self.source_dir.join(name))
                    .collect(),
                false,
            ),
            language => {
                let std_flag = match language {
                    ccx_core::capsule::Language::C => "-std=c17",
                    _ => "-std=c++20",
                };
                (
                    vec![self.sol.opt_level.clone(), std_flag.to_string()],
                    self.sol
                        .sources
                        .keys()
                        .map(|name| self.source_dir.join(name))
                        .collect(),
                    false,
                )
            }
        };
        let mut flags = flags;
        flags.extend(self.sol.compiler_flags.iter().cloned());
        let (_, timing, _) =
            self.build_artifact(driver, &flags, &sources, &dir, "sol", with_shim)?;
        Ok((
            Program {
                host_dir: dir,
                guest_binary: format!("{GUEST_PROG}/sol"),
                runtime_argv: self.expand_runtime_argv("/prog/sol", &self.sol.program_args),
                runtime_prefix: if self.sol.language == ccx_core::capsule::Language::Go {
                    vec![
                        "env".to_string(),
                        "GOMAXPROCS=1".to_string(),
                        "GODEBUG=randautoseed=0".to_string(),
                    ]
                } else {
                    vec![]
                },
                args: self.sol.program_args.clone(),
                prefix: vec![],
            },
            timing,
        ))
    }

    pub fn build_solution_repeated(
        &self,
        repeat: u32,
    ) -> anyhow::Result<(Program, Vec<BuildTiming>, String)> {
        if repeat == 0 || repeat > 20 {
            return Err(Fixable::new(format!(
                "build repeat count {repeat} is outside the supported range"
            ))
            .fix("choose a build repeat count from 1 to 20")
            .into_error());
        }
        let mut program = None;
        let mut timings = Vec::with_capacity(repeat as usize);
        let mut digest = None;
        for build_index in 0..repeat {
            let (built_program, timing) = self.build_solution_timed()?;
            let current_digest =
                ccx_core::capsule::sha256_hex(&std::fs::read(built_program.host_dir.join("sol"))?);
            if let Some(expected) = &digest {
                if expected != &current_digest {
                    return Err(Fixable::new(format!(
                        "clean build {build_index} produced different executable bytes"
                    ))
                    .fix("make the build deterministic or use one pinned build configuration")
                    .into_error());
                }
            } else {
                digest = Some(current_digest);
            }
            timings.push(timing);
            program = Some(built_program);
        }
        Ok((
            program.ok_or_else(|| anyhow::anyhow!("no clean build was performed"))?,
            timings,
            digest.ok_or_else(|| anyhow::anyhow!("no executable digest was produced"))?,
        ))
    }

    pub fn build_assembly(&self) -> anyhow::Result<Vec<(String, PathBuf)>> {
        if self.sol.language == ccx_core::capsule::Language::AsmX86_64 {
            return self
                .sol
                .sources
                .keys()
                .next()
                .map(|name| vec![(name.clone(), self.source_dir.join(name))])
                .ok_or_else(|| anyhow::anyhow!("assembly solution has no source files"));
        }
        if self.sol.language == ccx_core::capsule::Language::Go {
            let program = self.build_solution()?;
            let mut policy = SandboxPolicy::default();
            policy.timeout_ms = 60_000;
            policy.memory_bytes = 4 * 1024 * 1024 * 1024;
            policy.container_memory_bytes = 1536 * 1024 * 1024;
            policy.max_output_bytes = 8 * 1024 * 1024;
            policy.scratch_tmpfs = "128m".to_string();
            policy.scratch_bytes = 128 * 1024 * 1024;
            policy.cpuset = None;
            let command = vec![
                "/opt/ccx/runner".to_string(),
                "--timeout-ms".to_string(),
                policy.timeout_ms.to_string(),
                "--memory-bytes".to_string(),
                policy.memory_bytes.to_string(),
                "--stack-bytes".to_string(),
                "8388608".to_string(),
                "--pids-limit".to_string(),
                policy.pids_limit.to_string(),
                "--max-output-bytes".to_string(),
                policy.max_output_bytes.to_string(),
                "--cwd".to_string(),
                "/work/scratch".to_string(),
                "--".to_string(),
                "env".to_string(),
                "GOMAXPROCS=1".to_string(),
                "GOENV=off".to_string(),
                "GOROOT=/usr/local/go".to_string(),
                "GOCACHE=/work/scratch/go-cache".to_string(),
                "GOTMPDIR=/work/scratch".to_string(),
                "go".to_string(),
                "tool".to_string(),
                "objdump".to_string(),
                "-S".to_string(),
                "-s".to_string(),
                "^main\\.".to_string(),
                "/prog/sol".to_string(),
            ];
            let argv = policy.argv_mounted(
                &self.image,
                &command,
                &[(
                    program.host_dir.to_string_lossy().to_string(),
                    "/prog".to_string(),
                )],
            );
            let (_, stdout, stderr) = self.docker.run(&argv)?;
            let report = parse_framed_report(stdout.as_bytes()).ok_or_else(|| {
                Fixable::new(format!(
                    "Go binary disassembly produced no usable report\n{stderr}"
                ))
                .fix("confirm the pinned toolchain image contains `go tool objdump`")
                .into_error()
            })?;
            if report.exit_status != 0 {
                return Err(Fixable::new(format!(
                    "Go binary disassembly failed (exit {})\n{}",
                    report.exit_status, report.stderr
                ))
                .fix("confirm the pinned toolchain image contains `go tool objdump`")
                .into_error());
            }
            let path = self.assembly_output_path();
            std::fs::create_dir_all(self.sol.out_dir())?;
            std::fs::write(&path, report.stdout)?;
            return Ok(vec![("binary".to_string(), path)]);
        }
        let driver = ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let (flags, sources) = match self.sol.language {
            ccx_core::capsule::Language::Rust => {
                let entry = if self.source_dir.join("main.rs").is_file() {
                    self.source_dir.join("main.rs")
                } else {
                    self.source_dir
                        .join(self.sol.sources.keys().next().unwrap())
                };
                let opt = self.sol.opt_level.trim_start_matches("-O");
                (
                    vec![
                        "--edition=2021".to_string(),
                        format!("-Copt-level={opt}"),
                        "--emit=asm".to_string(),
                    ],
                    vec![entry],
                )
            }
            ccx_core::capsule::Language::D => (
                vec![
                    self.sol.opt_level.clone(),
                    "-S".to_string(),
                    "-fverbose-asm".to_string(),
                    "-I/src0".to_string(),
                ],
                self.sol
                    .sources
                    .keys()
                    .map(|name| self.source_dir.join(name))
                    .collect(),
            ),
            ccx_core::capsule::Language::Zig => {
                let mode = match self.sol.opt_level.as_str() {
                    "-O0" => "Debug",
                    "-Os" | "-Oz" => "ReleaseSmall",
                    _ => "ReleaseFast",
                };
                let mut flags = vec![
                    "build-exe".to_string(),
                    format!("-O{mode}"),
                    "-lc".to_string(),
                ];
                flags.extend(zig_target_flags());
                (
                    flags,
                    vec![if self.source_dir.join("main.zig").is_file() {
                        self.source_dir.join("main.zig")
                    } else {
                        self.source_dir
                            .join(self.sol.sources.keys().next().unwrap())
                    }],
                )
            }
            ccx_core::capsule::Language::Odin => (
                vec![
                    "build".to_string(),
                    "-build-mode:assembly".to_string(),
                    "-o:speed".to_string(),
                    "-out:/work/scratch/out.s".to_string(),
                ],
                vec![self.source_dir.clone()],
            ),
            language => {
                let mut flags = vec![
                    self.sol.opt_level.clone(),
                    "-fverbose-asm".to_string(),
                    "-S".to_string(),
                ];
                if language == ccx_core::capsule::Language::C {
                    flags.extend(["-std=c17".to_string(), "-masm=intel".to_string()]);
                } else if language == ccx_core::capsule::Language::D {
                    flags.push("-masm=intel".to_string());
                } else {
                    flags.extend(["-std=c++20".to_string(), "-masm=intel".to_string()]);
                }
                (
                    flags,
                    self.sol
                        .sources
                        .keys()
                        .map(|name| self.source_dir.join(name))
                        .collect(),
                )
            }
        };
        let mut flags = flags;
        flags.extend(self.sol.compiler_flags.iter().cloned());
        if sources.len() > 1
            && matches!(
                self.sol.language,
                ccx_core::capsule::Language::C
                    | ccx_core::capsule::Language::Cpp
                    | ccx_core::capsule::Language::D
            )
        {
            let run_name = self
                .work_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("run");
            let output_dir = self.sol.out_dir().join(format!("{run_name}-asm"));
            let mut outputs = Vec::new();
            for (index, source) in sources.iter().enumerate() {
                let (generated, _, _) = self.build_artifact(
                    driver,
                    &flags,
                    std::slice::from_ref(source),
                    &self.scratch_dir().join(format!("assembly-{index}")),
                    "out.s",
                    false,
                )?;
                let relative = source.strip_prefix(&self.source_dir)?;
                let mut output = output_dir.join(relative);
                output.set_extension("s");
                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(generated, &output)?;
                outputs.push((relative.to_string_lossy().to_string(), output));
            }
            return Ok(outputs);
        }
        let (generated, _, _) = self.build_artifact(
            driver,
            &flags,
            &sources,
            &self.scratch_dir().join("assembly"),
            "out.s",
            false,
        )?;
        let output = self.assembly_output_path();
        std::fs::create_dir_all(self.sol.out_dir())?;
        std::fs::copy(generated, &output)?;
        let source = sources
            .first()
            .and_then(|source| source.strip_prefix(&self.source_dir).ok())
            .map(|relative| {
                if relative.as_os_str().is_empty() {
                    "package".to_string()
                } else {
                    relative.to_string_lossy().to_string()
                }
            })
            .unwrap_or_else(|| "solution".to_string());
        Ok(vec![(source, output)])
    }

    pub fn optimization_remarks(&self) -> anyhow::Result<OptimizationRemarks> {
        use ccx_core::capsule::Language;

        let unsupported = |explanation: &str| OptimizationRemarks {
            status: "unavailable".to_string(),
            method: None,
            explanation: Some(explanation.to_string()),
            text: String::new(),
        };
        let (compiler, method, source_filters) = match self.sol.language {
            Language::Rust => {
                return Ok(unsupported(
                    "this rustc profile does not expose stable optimization remarks",
                ));
            }
            Language::Zig => {
                return Ok(unsupported(
                    "this Zig profile does not expose stable optimization remarks",
                ));
            }
            Language::Odin => {
                return Ok(unsupported(
                    "this Odin profile does not expose optimization remarks",
                ));
            }
            Language::AsmX86_64 => {
                return Ok(unsupported(
                    "handwritten assembly has no compiler optimization pass",
                ));
            }
            Language::Go => {
                let gcflags = if self.sol.opt_level == "-O0" {
                    "command-line-arguments=-m=2 -N"
                } else {
                    "command-line-arguments=-m=2"
                };
                let mut compiler = vec![
                    "env".to_string(),
                    "GO111MODULE=off".to_string(),
                    "GOTOOLCHAIN=local".to_string(),
                    "GOPROXY=off".to_string(),
                    "GOSUMDB=off".to_string(),
                    "GOENV=off".to_string(),
                    "GOROOT=/usr/local/go".to_string(),
                    "GOCACHE=/work/scratch/go-cache".to_string(),
                    "GOTMPDIR=/work/scratch".to_string(),
                    "go".to_string(),
                    "build".to_string(),
                    "-trimpath".to_string(),
                    "-buildvcs=false".to_string(),
                    format!("-gcflags={gcflags}"),
                    "-o".to_string(),
                    "/dev/null".to_string(),
                ];
                compiler.extend(self.sol.compiler_flags.iter().cloned());
                compiler.extend(self.sol.sources.keys().map(|name| format!("/src0/{name}")));
                (
                    compiler,
                    "Go compiler inlining and escape analysis (-m=2)".to_string(),
                    self.sol
                        .sources
                        .keys()
                        .map(|name| format!("./{name}"))
                        .collect(),
                )
            }
            Language::D => {
                let mut compiler = vec![
                    ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language)
                        .to_string(),
                    self.sol.opt_level.clone(),
                    "-fopt-info-all".to_string(),
                    "-o".to_string(),
                    "/dev/null".to_string(),
                ];
                compiler.extend(self.sol.compiler_flags.iter().cloned());
                compiler.extend(self.sol.sources.keys().map(|name| format!("/src0/{name}")));
                (
                    compiler,
                    "GDC -fopt-info-all".to_string(),
                    vec!["/src0/".to_string()],
                )
            }
            _ => {
                let mut compiler = vec![
                    ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language)
                        .to_string(),
                    self.sol.opt_level.clone(),
                ];
                let method = if matches!(
                    self.sol.toolchain_name,
                    ccx_core::capsule::ToolchainName::Clang
                        | ccx_core::capsule::ToolchainName::Clang18
                ) {
                    compiler.extend([
                        "-Rpass=.*".to_string(),
                        "-Rpass-missed=.*".to_string(),
                        "-Rpass-analysis=.*".to_string(),
                    ]);
                    "Clang optimization remarks".to_string()
                } else {
                    compiler.push("-fopt-info-all".to_string());
                    "GCC -fopt-info-all".to_string()
                };
                compiler.extend(self.sol.compiler_flags.iter().cloned());
                compiler.extend(["-o".to_string(), "/dev/null".to_string()]);
                compiler.extend(self.sol.sources.keys().map(|name| format!("/src0/{name}")));
                (compiler, method, vec!["/src0/".to_string()])
            }
        };
        let mut command = vec!["/opt/ccx/runner".to_string()];
        command.extend([
            "--timeout-ms".to_string(),
            "120000".to_string(),
            "--memory-bytes".to_string(),
            "2147483648".to_string(),
            "--stack-bytes".to_string(),
            "8388608".to_string(),
            "--pids-limit".to_string(),
            "64".to_string(),
            "--max-output-bytes".to_string(),
            (8 * 1024 * 1024u64).to_string(),
            "--cwd".to_string(),
            "/work/scratch".to_string(),
            "--".to_string(),
        ]);
        command.extend(compiler);
        let mounts: Vec<(String, String)> = vec![(
            self.source_dir.to_string_lossy().to_string(),
            "/src0".to_string(),
        )];
        let policy = SandboxPolicy {
            memory_bytes: 2 * 1024 * 1024 * 1024,
            container_memory_bytes: 3 * 1024 * 1024 * 1024,
            timeout_ms: 120_000,
            cpuset: None,
            ..SandboxPolicy::default()
        };
        let mut policy = policy;
        if self.sol.language == Language::Go {
            policy.scratch_bytes = 2 * 1024 * 1024 * 1024;
            policy.scratch_tmpfs = "2g".to_string();
        }
        let argv = policy.argv_mounted(&self.image, &command, &mounts);
        let (_, stdout, stderr) = self.docker.run(&argv)?;
        let outcome = parse_framed_report(stdout.as_bytes()).ok_or_else(|| {
            Fixable::new(format!(
                "the compiler remarks pass produced no usable report\n{stderr}"
            ))
            .fix("run `ccx build` first and report this error if compilation succeeds")
            .into_error()
        })?;
        if outcome.exit_status != 0 {
            return Err(Fixable::new(format!(
                "compiler remarks pass failed (exit {})\n{}",
                outcome.exit_status, outcome.stderr
            ))
            .fix("run `ccx build` first: the same errors will be clearer")
            .into_error());
        }
        let text = filter_optimization_remarks(&outcome.stderr, &source_filters);
        let explanation = text.is_empty().then(|| {
            "the compiler completed successfully but emitted no remarks for the solution sources"
                .to_string()
        });
        Ok(OptimizationRemarks {
            status: "available".to_string(),
            method: Some(method),
            explanation,
            text,
        })
    }

    /// The exact compiler argv used for the solution, for the capsule.
    ///
    /// A capsule has to record how the binary was produced, or a replay cannot
    /// rebuild the same bytes. The argv names guest paths (`/src0`, ...), which
    /// is what actually ran; the host directories are mounts, not arguments.
    fn custom_build_argv(&self) -> Vec<String> {
        let driver = ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let standard = if self.sol.language == ccx_core::capsule::Language::C {
            "-std=c17"
        } else {
            "-std=c++20"
        };
        let mut flags = vec![
            self.sol.opt_level.clone(),
            standard.to_string(),
            "-ffile-prefix-map=/in=.".to_string(),
            "-fno-ident".to_string(),
            "-Wl,--build-id=none".to_string(),
        ];
        flags.extend(self.sol.compiler_flags.iter().cloned());
        let sources = self
            .sol
            .sources
            .keys()
            .map(|name| format!("/src0/{name}"))
            .collect::<Vec<_>>();
        let mut argv = Vec::new();
        for token in &self.sol.build_command {
            match token.as_str() {
                "{compiler}" => argv.push(driver.to_string()),
                "{flags}" => argv.extend(flags.iter().cloned()),
                "{sources}" => argv.extend(sources.iter().cloned()),
                "{output}" => argv.push("/work/scratch/sol".to_string()),
                value => argv.push(value.to_string()),
            }
        }
        argv
    }

    pub fn build_argv(&self) -> Vec<String> {
        if self.sol.build_system == BuildSystem::Custom {
            return self.custom_build_argv();
        }
        if self.sol.build_system == BuildSystem::Cmake {
            return vec![
                "sh".to_string(),
                "-eu".to_string(),
                "-c".to_string(),
                self.cmake_script(),
            ];
        }
        let driver = ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let names: Vec<String> = if self.sol.language == ccx_core::capsule::Language::Go {
            self.sol.sources.keys().cloned().collect()
        } else if self.sol.language == ccx_core::capsule::Language::Odin {
            vec![".".to_string()]
        } else if matches!(
            self.sol.language,
            ccx_core::capsule::Language::Rust | ccx_core::capsule::Language::Zig
        ) {
            vec![if self.sol.language == ccx_core::capsule::Language::Rust
                && self.source_dir.join("main.rs").is_file()
            {
                "main.rs".to_string()
            } else if self.sol.language == ccx_core::capsule::Language::Zig
                && self.source_dir.join("main.zig").is_file()
            {
                "main.zig".to_string()
            } else if self.sol.language == ccx_core::capsule::Language::Odin
                && self.source_dir.join("main.odin").is_file()
            {
                ".".to_string()
            } else {
                self.sol.sources.keys().next().unwrap().clone()
            }]
        } else {
            self.sol.sources.keys().cloned().collect()
        };
        let mut argv = if self.sol.language == ccx_core::capsule::Language::Go {
            vec![
                "env".to_string(),
                "GO111MODULE=off".to_string(),
                "GOTOOLCHAIN=local".to_string(),
                "GOPROXY=off".to_string(),
                "GOSUMDB=off".to_string(),
                "GOENV=off".to_string(),
                "GOROOT=/usr/local/go".to_string(),
                "GOCACHE=/work/scratch/go-cache".to_string(),
                "GOTMPDIR=/work/scratch".to_string(),
                "go".to_string(),
            ]
        } else {
            vec![driver.to_string()]
        };
        match self.sol.language {
            ccx_core::capsule::Language::Rust => {
                let opt = self.sol.opt_level.trim_start_matches("-O");
                argv.extend(["--edition=2021".to_string(), format!("-Copt-level={opt}")]);
                argv.push("--remap-path-prefix=/src0=.".to_string());
            }
            ccx_core::capsule::Language::D => {
                argv.push(self.sol.opt_level.clone());
                argv.extend([
                    "-ffile-prefix-map=/in=.".to_string(),
                    "-fno-ident".to_string(),
                    "-Wl,--build-id=none".to_string(),
                ]);
            }
            ccx_core::capsule::Language::Zig => {
                let mode = match self.sol.opt_level.as_str() {
                    "-O0" => "Debug",
                    "-Os" | "-Oz" => "ReleaseSmall",
                    _ => "ReleaseFast",
                };
                argv.extend([
                    "build-exe".to_string(),
                    format!("-O{mode}"),
                    "-lc".to_string(),
                ]);
                argv.extend(zig_target_flags());
                argv.extend([
                    "--cache-dir".to_string(),
                    "/work/scratch/zig-cache".to_string(),
                    "--global-cache-dir".to_string(),
                    "/work/scratch/zig-global-cache".to_string(),
                    "-femit-bin=/work/scratch/sol".to_string(),
                ]);
            }
            ccx_core::capsule::Language::Odin => {
                let optimization = if self.sol.opt_level == "-O0" {
                    "-debug"
                } else {
                    "-o:speed"
                };
                argv.extend([
                    "build".to_string(),
                    optimization.to_string(),
                    "-out:/work/scratch/sol".to_string(),
                ]);
            }
            ccx_core::capsule::Language::Go => {
                argv.extend([
                    "build".to_string(),
                    "-trimpath".to_string(),
                    "-buildvcs=false".to_string(),
                    "-ldflags=-buildid=".to_string(),
                ]);
                if self.sol.opt_level == "-O0" {
                    argv.push("-gcflags=all=-N".to_string());
                }
                argv.extend(["-o".to_string(), "/work/scratch/sol".to_string()]);
            }
            ccx_core::capsule::Language::AsmX86_64 => {
                argv.extend([
                    "-ffile-prefix-map=/in=.".to_string(),
                    "-fno-ident".to_string(),
                ]);
                argv.push("-Wl,--build-id=none".to_string());
            }
            ccx_core::capsule::Language::C => {
                argv.extend([self.sol.opt_level.clone(), "-std=c17".to_string()]);
                argv.extend([
                    "-ffile-prefix-map=/in=.".to_string(),
                    "-fno-ident".to_string(),
                    "-Wl,--build-id=none".to_string(),
                ]);
            }
            _ => {
                argv.extend([self.sol.opt_level.clone(), "-std=c++20".to_string()]);
                argv.extend([
                    "-ffile-prefix-map=/in=.".to_string(),
                    "-fno-ident".to_string(),
                    "-Wl,--build-id=none".to_string(),
                ]);
            }
        }
        argv.extend(self.sol.compiler_flags.iter().cloned());
        if !matches!(
            self.sol.language,
            ccx_core::capsule::Language::Zig
                | ccx_core::capsule::Language::Odin
                | ccx_core::capsule::Language::Go
        ) {
            argv.extend(["-o".to_string(), "/work/scratch/sol".to_string()]);
        }
        // Sources are mounted one directory at a time, in the order the solution
        // declares them, so the argv is stable across runs.
        for name in &names {
            argv.push(format!("/src0/{name}"));
        }
        argv
    }

    /// Run a program under the runner and return its outcome.
    ///
    /// This is the only place a program is executed. Both plain and measured
    /// runs go through it, so containment is identical for both.
    pub fn run_program(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
        measured: bool,
    ) -> anyhow::Result<RunOutcome> {
        self.run_program_inner(prog, input, policy, measured, false, false)
    }

    fn run_program_with_hardware(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
    ) -> anyhow::Result<RunOutcome> {
        self.run_program_inner(prog, input, policy, false, true, false)
    }

    fn run_program_with_allocations(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
    ) -> anyhow::Result<RunOutcome> {
        let mut profiled = prog.clone();
        profiled.prefix = vec![
            "valgrind".to_string(),
            "--tool=memcheck".to_string(),
            "--xtree-memory=full".to_string(),
            format!("--xtree-memory-file={GUEST_ALLOC}"),
        ];
        self.run_program_inner(&profiled, input, policy, true, false, true)
    }

    fn run_program_inner(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
        measured: bool,
        collect_hardware: bool,
        collect_allocations: bool,
    ) -> anyhow::Result<RunOutcome> {
        let input_dir = self.create_input_dir()?;
        let outcome = self.run_program_in_dir(
            prog,
            input,
            policy,
            measured,
            collect_hardware,
            collect_allocations,
            &input_dir,
        );
        let _ = std::fs::remove_dir_all(&input_dir);
        outcome
    }

    fn run_program_in_dir(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
        measured: bool,
        collect_hardware: bool,
        collect_allocations: bool,
        input_dir: &Path,
    ) -> anyhow::Result<RunOutcome> {
        std::fs::write(input_dir.join("stdin.txt"), input)?;

        let address_space_limit = if self.sol.language == ccx_core::capsule::Language::Go
            && prog.guest_binary == format!("{GUEST_PROG}/sol")
        {
            8 * 1024 * 1024 * 1024u64
        } else {
            policy.memory_bytes
        };
        let mut command: Vec<String> = vec!["/opt/ccx/runner".to_string()];
        command.extend([
            "--timeout-ms".to_string(),
            policy.timeout_ms.to_string(),
            "--memory-bytes".to_string(),
            address_space_limit.to_string(),
            "--stack-bytes".to_string(),
            "8388608".to_string(),
            "--pids-limit".to_string(),
            policy.pids_limit.to_string(),
            "--max-output-bytes".to_string(),
            policy.max_output_bytes.to_string(),
            "--stdin".to_string(),
            GUEST_STDIN.to_string(),
        ]);
        if collect_allocations {
            command.extend(["--alloc-xtree".to_string(), GUEST_ALLOC.to_string()]);
        }
        if collect_hardware {
            command.push("--hardware".to_string());
        }
        command.push("--".to_string());
        // The runner passes everything after `--` straight to execv, so a
        // prefix (valgrind) and program arguments need no shell.
        command.extend(prog.prefix.iter().cloned());
        command.extend(prog.command_argv());

        let mut run_policy = policy.clone();
        if address_space_limit != policy.memory_bytes {
            run_policy.container_memory_bytes =
                policy.memory_bytes.saturating_add(64 * 1024 * 1024);
        }
        if measured {
            // Valgrind is roughly fifty times slower than native, so a measured
            // run needs a wall clock sized for the simulator, not the machine.
            run_policy.timeout_ms = policy.timeout_ms.saturating_mul(60).max(60_000);
        }

        let argv = run_policy.argv_mounted(
            &self.image,
            &command,
            &[
                (
                    prog.host_dir.to_string_lossy().to_string(),
                    GUEST_PROG.to_string(),
                ),
                (
                    input_dir.to_string_lossy().to_string(),
                    GUEST_INPUT.to_string(),
                ),
            ],
        );

        let (_, stdout, stderr) = self.docker.run(&argv)?;
        let bytes = stdout.into_bytes();
        parse_framed_report(&bytes).ok_or_else(|| {
            Fixable::new(format!(
                "the runner produced no usable report\n\nstderr:\n{stderr}"
            ))
            .fix("this is an engine bug: report it with the problem id")
            .into_error()
        })
    }

    /// Generate one test input for a named size and seed.
    ///
    /// Builds the generator first; callers looping over many cases should build
    /// it once with `build_generator` and call `generate_with` instead, because
    /// a container start per case dominates the run time.
    pub fn generate(&self, size: &str, seed: u64) -> anyhow::Result<Vec<u8>> {
        let gen = self.build_generator()?;
        self.generate_with(&gen, size, seed)
    }

    /// Generate one test input using an already-built generator.
    pub fn generate_with(&self, gen: &Program, size: &str, seed: u64) -> anyhow::Result<Vec<u8>> {
        let scale = *self.sol.problem.meta.sizes.get(size).ok_or_else(|| {
            Fixable::new(format!("problem has no size named {size:?}"))
                .fix(format!(
                    "available sizes: {}",
                    self.sol
                        .problem
                        .meta
                        .sizes
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
                .into_error()
        })?;

        let mut gen = gen.clone();
        // The Phase 1 generator convention is `seed scale` on argv.
        gen.args = vec![seed.to_string(), scale.to_string()];

        let mut policy = policy_for(&self.sol.problem);
        // A generator legitimately emits large input; its output cap is the
        // only limit that differs from a solution's.
        policy.max_output_bytes = 256 * 1024 * 1024;
        let outcome = self.run_program(&gen, &[], &policy, false)?;
        if outcome.exit_status != 0 {
            return Err(Fixable::new(format!(
                "the input generator failed (exit {}, signal {})",
                outcome.exit_status, outcome.signal
            ))
            .fix("check generator.c takes `seed scale` and writes to stdout")
            .into_error());
        }
        Ok(outcome.stdout.into_bytes())
    }

    /// Produce the reference answer for one input.
    pub fn reference_output(&self, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        let prog = self.build_reference()?;
        self.reference_output_with(&prog, input)
    }

    /// Produce the reference answer using an already-built reference.
    pub fn reference_output_with(&self, prog: &Program, input: &[u8]) -> anyhow::Result<Vec<u8>> {
        let policy = policy_for(&self.sol.problem);
        let outcome = self.run_program(prog, input, &policy, false)?;
        if outcome.exit_status != 0 {
            return Err(Fixable::new(format!(
                "the reference solution failed (exit {}, signal {})",
                outcome.exit_status, outcome.signal
            ))
            .fix("check reference.cpp is a correct solution")
            .into_error());
        }
        Ok(outcome.stdout.into_bytes())
    }

    /// Measure one run: a plain run for rusage numbers, a Cachegrind run for
    /// simulated counters, and a Memcheck profile for allocation counts.
    pub fn measure_once(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
    ) -> anyhow::Result<(RunOutcome, Option<ccx_core::measure::CachegrindCounts>)> {
        let outcome = self.run_program_with_hardware(prog, input, policy)?;
        self.measure_outcome(prog, input, policy, outcome)
    }

    pub fn measure_once_paired(
        &self,
        prog: &Program,
        reference: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
        solution_first: bool,
    ) -> anyhow::Result<(
        RunOutcome,
        Option<ccx_core::measure::CachegrindCounts>,
        RunOutcome,
    )> {
        let (solution_outcome, reference_outcome) = if solution_first {
            let solution_outcome = self.run_program_with_hardware(prog, input, policy)?;
            let reference_outcome = self.run_program_with_hardware(reference, input, policy)?;
            (solution_outcome, reference_outcome)
        } else {
            let reference_outcome = self.run_program_with_hardware(reference, input, policy)?;
            let solution_outcome = self.run_program_with_hardware(prog, input, policy)?;
            (solution_outcome, reference_outcome)
        };
        let (solution_outcome, counts) =
            self.measure_outcome(prog, input, policy, solution_outcome)?;
        Ok((solution_outcome, counts, reference_outcome))
    }

    fn measure_outcome(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
        mut outcome: RunOutcome,
    ) -> anyhow::Result<(RunOutcome, Option<ccx_core::measure::CachegrindCounts>)> {
        if outcome.exit_status != 0 {
            return Ok((outcome, None));
        }

        let mut measured = prog.clone();
        // No `--quiet`: it suppresses the very summary this reads. The parser
        // matches specific labelled lines, so valgrind's other output is
        // ignored rather than having to be silenced.
        measured.prefix = std::iter::once("valgrind".to_string())
            .chain(
                CACHEGRIND_ARGS
                    .iter()
                    .map(|argument| (*argument).to_string()),
            )
            .collect();
        let measured_outcome = self.run_program(&measured, input, policy, true)?;
        if measured_outcome.exit_status != 0 || measured_outcome.signal != 0 {
            return Err(Fixable::new(format!(
                "Cachegrind measurement failed (exit {}, signal {})\n{}",
                measured_outcome.exit_status, measured_outcome.signal, measured_outcome.stderr
            ))
            .fix("check that the selected runtime command can run under Valgrind in the pinned image")
            .into_error());
        }
        let counts = Some(
            parse_cachegrind_summary(&measured_outcome.stderr).map_err(|error| {
                Fixable::new(format!(
                    "Cachegrind produced no usable counters: {error}\n{}",
                    measured_outcome.stderr
                ))
                .fix("run `ccx doctor` and ensure the pinned image includes a working Valgrind")
                .into_error()
            })?,
        );
        if matches!(
            self.sol.language,
            ccx_core::capsule::Language::C
                | ccx_core::capsule::Language::Cpp
                | ccx_core::capsule::Language::Rust
        ) {
            let allocation_outcome = self.run_program_with_allocations(prog, input, policy)?;
            if allocation_outcome.exit_status != 0 || allocation_outcome.signal != 0 {
                return Err(Fixable::new(format!(
                    "Memcheck allocation measurement failed (exit {}, signal {})\n{}",
                    allocation_outcome.exit_status,
                    allocation_outcome.signal,
                    allocation_outcome.stderr
                ))
                .fix("check that the selected runtime command can run under Memcheck in the pinned image")
                .into_error());
            }
            outcome.allocations = allocation_outcome.allocations;
            if outcome.allocations.is_none() {
                return Err(Fixable::new("Memcheck produced no usable allocation profile")
                    .fix("run `ccx doctor` and ensure the pinned image includes a working Valgrind")
                    .into_error());
            }
        }
        Ok((outcome, counts))
    }
}

#[cfg(test)]
mod optimization_remark_tests {
    use super::filter_optimization_remarks;

    #[test]
    fn optimization_remarks_are_limited_to_submission_sources() {
        let stderr = "/usr/lib/compiler/header.h:4: missed optimization\n/src0/main.cpp:8: loop vectorized\n/src0/lib.cpp:3: inline candidate";
        assert_eq!(
            filter_optimization_remarks(stderr, &["/src0/".to_string()]),
            "/src0/main.cpp:8: loop vectorized\n/src0/lib.cpp:3: inline candidate"
        );
    }

    #[test]
    fn go_optimization_remarks_match_trimmed_source_paths() {
        let stderr = "sync/atomic/type.go:4: standard library diagnostic\n./main.go:11: cannot inline nextInt";
        assert_eq!(
            filter_optimization_remarks(stderr, &["./main.go".to_string()]),
            "./main.go:11: cannot inline nextInt"
        );
    }

    #[test]
    fn empty_source_remarks_are_reported_as_empty() {
        assert!(filter_optimization_remarks(
            "compiler produced no diagnostics",
            &["/src0/".to_string()]
        )
        .is_empty());
    }
}

impl Drop for Pipeline<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }
}

#[cfg(test)]
mod tests {
    use super::{zig_target_flags, Program, CACHEGRIND_ARGS};
    use std::path::PathBuf;

    #[test]
    fn default_program_argv_keeps_environment_prefix_binary_and_arguments() {
        let program = Program {
            host_dir: PathBuf::new(),
            guest_binary: "/prog/sol".into(),
            args: vec!["input.txt".into()],
            runtime_argv: None,
            runtime_prefix: vec!["env".into(), "GOMAXPROCS=1".into()],
            prefix: Vec::new(),
        };
        assert_eq!(
            program.command_argv(),
            vec!["env", "GOMAXPROCS=1", "/prog/sol", "input.txt"]
        );
    }

    #[test]
    fn cachegrind_uses_the_fixed_cache_model() {
        assert!(CACHEGRIND_ARGS.contains(&"--I1=32768,8,64"));
        assert!(CACHEGRIND_ARGS.contains(&"--D1=32768,8,64"));
        assert!(!CACHEGRIND_ARGS
            .iter()
            .any(|argument| argument.starts_with("--LL=")));
    }

    #[test]
    fn zig_builds_pin_linux_x86_64_baseline_target() {
        assert_eq!(
            zig_target_flags(),
            vec!["-target", "x86_64-linux-gnu", "-mcpu=baseline"]
        );
    }

    #[test]
    fn custom_program_argv_is_preserved_for_single_and_batched_runs() {
        let program = Program {
            host_dir: PathBuf::new(),
            guest_binary: "/prog/sol".into(),
            args: vec!["ignored".into()],
            runtime_argv: Some(vec![
                "env".into(),
                "CCX_RUN=1".into(),
                "/prog/sol".into(),
                "--mode".into(),
            ]),
            runtime_prefix: Vec::new(),
            prefix: Vec::new(),
        };
        assert_eq!(
            program.command_argv(),
            vec!["env", "CCX_RUN=1", "/prog/sol", "--mode"]
        );
    }
}
