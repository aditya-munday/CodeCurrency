//! Shared pipeline steps: build artifacts, generate input, run programs.
//!
//! `test` and `measure` are both sequences of these steps, so they live here
//! once rather than being reimplemented per command. Every step keeps the same
//! boundary discipline as `build`: read-only mounts, tmpfs scratch, artifacts
//! copied out, and untrusted programs only ever run under the runner.

use crate::cmd::Fixable;
use crate::context::{policy_for, Solution};
use ccx_core::engine::{parse_framed_report, Docker, RunOutcome};
use ccx_core::measure::parse_cachegrind_summary;
use ccx_core::sandbox::SandboxPolicy;
use std::path::{Path, PathBuf};

/// Source-file extension for a language, for callers that only need the name.
pub fn extension_for(language: ccx_core::capsule::Language) -> &'static str {
    ccx_core::toolchain::extension(language)
}

/// Guest path where a compiled program's directory is mounted read-only.
const GUEST_PROG: &str = "/prog";
/// Guest path where test input is mounted read-only.
const GUEST_INPUT: &str = "/input";
/// Guest path the runner reads as the program's stdin.
const GUEST_STDIN: &str = "/input/stdin.txt";
/// Guest path the allocation shim writes its accounting to.
const GUEST_ALLOC: &str = "/work/scratch/alloc.json";

/// A compiled program and how to invoke it inside the sandbox.
#[derive(Debug, Clone)]
pub struct Program {
    /// Host directory holding the binary, mounted read-only at `/prog`.
    pub host_dir: PathBuf,
    /// Guest path of the binary itself.
    pub guest_binary: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// Command prefix placed before the binary, e.g. `valgrind`. Empty for a
    /// plain run.
    pub prefix: Vec<String>,
}

pub struct Pipeline<'a> {
    pub sol: &'a Solution,
    pub docker: Docker,
    pub image: String,
}

impl<'a> Pipeline<'a> {
    pub fn new(sol: &'a Solution) -> anyhow::Result<Self> {
        let docker = crate::context::require_docker()?;
        let tc = sol.toolchain()?;
        Ok(Pipeline {
            sol,
            docker,
            image: format!("{}@{}", tc.image, tc.image_digest),
        })
    }

    fn scratch_dir(&self) -> PathBuf {
        self.sol.work_dir()
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
    ) -> anyhow::Result<PathBuf> {
        std::fs::create_dir_all(host_dir)?;

        let mut compiler: Vec<String> = vec![driver.to_string()];
        compiler.extend(std_flags.iter().cloned());
        compiler.extend([
            "-ffile-prefix-map=/in=.".to_string(),
            "-fno-ident".to_string(),
            "-Wl,--build-id=none".to_string(),
            "-o".to_string(),
            format!("/work/scratch/{out_name}"),
        ]);

        // Mount each distinct source directory read-only at its own mount point,
        // so two files with the same name in different folders cannot collide.
        let mut mounts: Vec<(String, String)> = Vec::new();
        for src in sources {
            let parent = src.parent().unwrap_or(Path::new(".")).to_path_buf();
            let name = src
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("bad source path {}", src.display()))?;
            let guest = format!("/src{}", mounts.len());
            compiler.push(format!("{guest}/{name}"));
            let host = parent.to_string_lossy().to_string();
            if !mounts.iter().any(|(h, _)| h == &host) {
                mounts.push((host, guest));
            }
        }
        if with_shim {
            // `--wrap` only rewrites calls in objects we link, so allocations
            // made inside a *shared* libstdc++ are invisible to the shim and
            // `std::vector` would report zero. Linking libstdc++/libgcc
            // statically puts those call sites in our link, which makes the
            // count real. This is a measurement requirement, not a
            // performance one: the reported number is wrong without it.
            compiler.extend([
                "-static-libstdc++".to_string(),
                "-static-libgcc".to_string(),
                "-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free".to_string(),
                "/opt/ccx/alloc_shim.o".to_string(),
            ]);
        }

        // The compiler is the runner's child, so it needs room to run: a large
        // translation unit is not a hostile infinite loop, but it is not fast
        // either. The wall clock is generous; the memory cap stays the default.
        let mut policy = SandboxPolicy::default();
        policy.timeout_ms = 300_000;
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
        let (outcome, emitted) =
            ccx_core::engine::parse_framed_report_with_emit(&stdout).ok_or_else(|| {
                Fixable::new(format!(
                    "the compiler produced no usable report\n\nstderr:\n{stderr}"
                ))
                .fix("this is an engine bug: report it with the problem id")
                .into_error()
            })?;

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
            .fix(format!(
                "the exact command was:\n  {}",
                compiler.join(" ")
            ))
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
        let mut perms = std::fs::metadata(&host_artifact)?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        std::fs::set_permissions(&host_artifact, perms)?;
        Ok(host_artifact)
    }

    /// Compile the problem's input generator (`generator.c`).
    ///
    /// Returns a `Program` whose guest path is where the run-time `/prog` mount
    /// exposes the binary — not where the build wrote it.
    pub fn build_generator(&self) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join("gen");
        self.build_artifact(
            "gcc",
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
            prefix: vec![],
        })
    }

    /// Compile the problem's reference solution (`reference.cpp`).
    pub fn build_reference(&self) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join("ref");
        self.build_artifact(
            "g++",
            &["-O2".to_string(), "-std=c++20".to_string()],
            &[self.sol.problem.root.join("reference.cpp")],
            &dir,
            "ref",
            false,
        )?;
        Ok(Program {
            host_dir: dir,
            guest_binary: format!("{GUEST_PROG}/ref"),
            args: vec![],
            prefix: vec![],
        })
    }

    /// Compile the user's solution. The binary is returned as a `Program`.
    pub fn build_solution(&self) -> anyhow::Result<Program> {
        let dir = self.scratch_dir().join("sol");
        let driver =
            ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let std_flag = match self.sol.language {
            ccx_core::capsule::Language::C => "-std=c17",
            _ => "-std=c++20",
        };
        let sources: Vec<PathBuf> = self
            .sol
            .sources
            .keys()
            .map(|n| self.sol.root.join(n))
            .collect();
        self.build_artifact(
            driver,
            &[self.sol.opt_level.clone(), std_flag.to_string()],
            &sources,
            &dir,
            "sol",
            true,
        )?;
        Ok(Program {
            host_dir: dir,
            guest_binary: format!("{GUEST_PROG}/sol"),
            args: vec![],
            prefix: vec![],
        })
    }

    /// The exact compiler argv used for the solution, for the capsule.
    ///
    /// A capsule has to record how the binary was produced, or a replay cannot
    /// rebuild the same bytes. The argv names guest paths (`/src0`, ...), which
    /// is what actually ran; the host directories are mounts, not arguments.
    pub fn build_argv(&self) -> Vec<String> {
        let driver =
            ccx_core::toolchain::driver_for(self.sol.toolchain_name, self.sol.language);
        let std_flag = match self.sol.language {
            ccx_core::capsule::Language::C => "-std=c17",
            _ => "-std=c++20",
        };
        let mut argv = vec![driver.to_string(), self.sol.opt_level.clone(), std_flag.to_string()];
        argv.extend([
            "-ffile-prefix-map=/in=.".to_string(),
            "-fno-ident".to_string(),
            "-Wl,--build-id=none".to_string(),
            "-o".to_string(),
            format!("{GUEST_PROG}/sol"),
        ]);
        // Sources are mounted one directory at a time, in the order the solution
        // declares them, so the argv is stable across runs.
        for (i, name) in self.sol.sources.keys().enumerate() {
            argv.push(format!("/src{i}/{name}"));
        }
        argv.extend([
            "-static-libstdc++".to_string(),
            "-static-libgcc".to_string(),
            "-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,--wrap=free".to_string(),
            "/opt/ccx/alloc_shim.o".to_string(),
        ]);
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
        let input_dir = self.scratch_dir().join("input");
        std::fs::create_dir_all(&input_dir)?;
        std::fs::write(input_dir.join("stdin.txt"), input)?;

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
            policy.max_output_bytes.to_string(),
            "--alloc-out".to_string(),
            GUEST_ALLOC.to_string(),
            "--stdin".to_string(),
            GUEST_STDIN.to_string(),
            "--".to_string(),
        ]);
        // The runner passes everything after `--` straight to execv, so a
        // prefix (valgrind) and program arguments need no shell.
        command.extend(prog.prefix.iter().cloned());
        command.push(prog.guest_binary.clone());
        command.extend(prog.args.iter().cloned());

        let mut run_policy = policy.clone();
        if measured {
            // Valgrind is roughly fifty times slower than native, so a measured
            // run needs a wall clock sized for the simulator, not the machine.
            run_policy.timeout_ms = policy.timeout_ms.saturating_mul(60).max(60_000);
        }

        let argv = run_policy.argv_mounted(
            &self.image,
            &command,
            &[
                (prog.host_dir.to_string_lossy().to_string(), GUEST_PROG.to_string()),
                (input_dir.to_string_lossy().to_string(), GUEST_INPUT.to_string()),
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

    /// Measure one run: a plain run for honest rusage numbers, plus a
    /// cachegrind run for simulated counters.
    ///
    /// The two are kept separate on purpose. The plain run gives peak memory
    /// and allocation counts; cachegrind gives instructions, cache misses, and
    /// branch misses. Mixing them would let simulated numbers claim to be
    /// measured.
    pub fn measure_once(
        &self,
        prog: &Program,
        input: &[u8],
        policy: &SandboxPolicy,
    ) -> anyhow::Result<(RunOutcome, Option<ccx_core::measure::CachegrindCounts>)> {
        let outcome = self.run_program(prog, input, policy, false)?;
        if outcome.exit_status != 0 {
            return Ok((outcome, None));
        }

        let mut measured = prog.clone();
        // No `--quiet`: it suppresses the very summary this reads. The parser
        // matches specific labelled lines, so valgrind's other output is
        // ignored rather than having to be silenced.
        measured.prefix = vec![
            "valgrind".to_string(),
            "--tool=cachegrind".to_string(),
            "--cache-sim=yes".to_string(),
            "--branch-sim=yes".to_string(),
            "--cachegrind-out-file=/dev/null".to_string(),
        ];
        let measured_outcome = self.run_program(&measured, input, policy, true)?;
        let counts = parse_cachegrind_summary(&measured_outcome.stderr).ok();
        Ok((outcome, counts))
    }
}
