//! CodeCurrency CLI (`ccx`).
//!
//! Layer 9 of Phase 1: one command surface for the whole engine, with a JSON
//! output mode for machines and readable tables for humans. Errors must say
//! what went wrong *and* how to fix it.

mod cmd;
mod context;
mod pipeline;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Global output format. Every command honours this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// Human-readable tables.
    Table,
    /// Machine-readable JSON.
    Json,
}

#[derive(Debug, Parser)]
#[command(
    name = "ccx",
    version,
    about = "CodeCurrency engine: build, test, measure, and replay performance experiments",
    long_about = "A performance arena for systems programmers.\n\n\
        Every result is explained, honest, and replayable. Deterministic metrics\n\
        (instructions, cache and branch misses) are the ranking signal; wall-clock\n\
        time is reported as indicative and is never used for ranking."
)]
pub struct Cli {
    /// Output format.
    #[arg(long, global = true, value_enum, default_value_t = Format::Table)]
    pub format: Format,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Check that Docker, the pinned toolchain images, and counters are available.
    Doctor,
    Profiles,
    /// Create a new solution scaffold in the current directory.
    Init {
        /// Problem id to target, e.g. `top-k/heap-vs-sort`.
        #[arg(long)]
        problem: Option<String>,
        /// Language for the scaffold.
        #[arg(long, default_value = "cpp")]
        language: String,
        #[arg(long, default_value = "auto")]
        toolchain: String,
        #[arg(long, default_value = "linux")]
        target_os: String,
        #[arg(long, default_value = "x86_64")]
        target_arch: String,
        /// Directory to create the scaffold in.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value = "direct", value_parser = ["direct", "cmake", "custom"])]
        build_system: String,
        #[arg(long, default_value = "ninja", value_parser = ["ninja", "make"])]
        cmake_generator: String,
        #[arg(long = "cmake-target", default_value = "ccx_solution")]
        cmake_target: String,
        #[arg(long, default_value = "standard", value_parser = ["standard", "custom"])]
        build_preset: String,
        #[arg(long)]
        compiler_version: Option<String>,
        #[arg(long = "compiler-flag", action = clap::ArgAction::Append, allow_hyphen_values = true)]
        compiler_flags: Vec<String>,
        #[arg(long = "program-arg", action = clap::ArgAction::Append, allow_hyphen_values = true)]
        program_args: Vec<String>,
        #[arg(long = "build-command-arg", action = clap::ArgAction::Append, allow_hyphen_values = true)]
        build_command: Vec<String>,
        #[arg(long = "run-command-arg", action = clap::ArgAction::Append, allow_hyphen_values = true)]
        run_command: Vec<String>,
    },
    /// Compile a solution inside the pinned image.
    Build {
        /// Directory holding the solution.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Build and run once, showing output. The debugging loop.
    Run {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Named problem size to generate.
        #[arg(long, default_value = "naive")]
        size: String,
        /// Seed for the generated input.
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Also print the reference output.
        #[arg(long)]
        show_expected: bool,
    },
    /// Run the correctness engine against the seeded test generators.
    Test {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Also run the hidden seeds, which is what defeats hard-coded answers.
        #[arg(long)]
        hidden: bool,
        #[arg(
            long,
            default_value_t = 0,
            help = "Run this many additional consecutive seeded cases (maximum 1,000,000)"
        )]
        cases: u64,
        #[arg(long, default_value_t = 1, help = "First seed for generated cases")]
        seed: u64,
        #[arg(
            long,
            help = "Size used for generated cases; defaults to the largest configured size"
        )]
        size: Option<String>,
        #[arg(
            long,
            default_value_t = 1,
            help = "Concurrent isolated case workers (1 to 16)"
        )]
        jobs: usize,
        #[arg(
            long,
            help = "Write every case result as JSON Lines, including cases omitted from the summary"
        )]
        case_log: Option<PathBuf>,
    },
    /// Measure deterministic metrics and emit a capsule.
    Measure {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Repeat the measurement N times to record the spread.
        #[arg(long, default_value_t = 20)]
        repeat: u32,
        /// Where to write the capsule (default: `capsule.json`).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Named problem size to measure.
        #[arg(long, default_value = "naive")]
        size: String,
        /// Seed for the generated input.
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    Experiment {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(long, default_value_t = 20)]
        repeat: u32,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long, default_value = "good")]
        size: String,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(
            long,
            default_value_t = 0,
            help = "Additional consecutive hidden cases, up to 1,000,000"
        )]
        cases: u64,
        #[arg(
            long,
            default_value_t = 1,
            help = "Concurrent isolated case workers (1 to 16)"
        )]
        jobs: usize,
        #[arg(long, help = "Write every test result as JSON Lines")]
        case_log: Option<PathBuf>,
    },
    Matrix {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(
            long = "toolchain",
            action = clap::ArgAction::Append,
            value_parser = ["gcc", "gcc-13", "clang", "clang-18", "rust", "gdc", "zig", "odin", "go"]
        )]
        toolchains: Vec<String>,
        #[arg(
            long = "opt-level",
            action = clap::ArgAction::Append,
            allow_hyphen_values = true
        )]
        opt_levels: Vec<String>,
        #[arg(
            long = "build-system",
            action = clap::ArgAction::Append,
            value_parser = ["direct", "cmake", "custom"]
        )]
        build_systems: Vec<String>,
        #[arg(
            long = "cmake-generator",
            action = clap::ArgAction::Append,
            value_parser = ["ninja", "make"]
        )]
        cmake_generators: Vec<String>,
        #[arg(long, default_value_t = 20)]
        repeat: u32,
        #[arg(long, default_value = "naive")]
        size: String,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long)]
        output_dir: Option<PathBuf>,
    },
    /// Emit assembly with source mapping and compiler remarks.
    Asm {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Diff two capsules: instructions, memory, branches, cache.
    Diff {
        /// Baseline capsule (the "before").
        a: PathBuf,
        /// Comparison capsule (the "after").
        b: PathBuf,
    },
    /// Re-run a capsule and report whether it matches.
    Replay {
        /// Capsule to replay.
        capsule: PathBuf,
        #[arg(long)]
        dir: Option<PathBuf>,
        /// Treat the replay as passing even if metrics differ (still reported).
        #[arg(long)]
        allow_drift: bool,
    },
    /// Validate a problem folder end to end.
    Validate {
        /// Problem folder.
        path: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match run(&cli) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    std::process::exit(code);
}

fn run(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Doctor => cmd::doctor::run(cli.format),
        Command::Profiles => cmd::profiles::run(cli.format),
        Command::Init {
            problem,
            language,
            toolchain,
            target_os,
            target_arch,
            dir,
            build_system,
            cmake_generator,
            cmake_target,
            build_preset,
            compiler_version,
            compiler_flags,
            program_args,
            build_command,
            run_command,
        } => cmd::init::run(
            cli.format,
            problem.as_deref(),
            language,
            toolchain,
            target_os,
            target_arch,
            build_system,
            cmake_generator,
            cmake_target,
            build_preset,
            compiler_version.as_deref(),
            compiler_flags,
            program_args,
            build_command,
            run_command,
            dir,
        ),
        Command::Build { dir } => cmd::build::run(cli.format, dir),
        Command::Run {
            dir,
            size,
            seed,
            show_expected,
        } => cmd::run::run(cli.format, dir, size, *seed, *show_expected),
        Command::Test {
            dir,
            hidden,
            cases,
            seed,
            size,
            jobs,
            case_log,
        } => cmd::test::run(
            cli.format,
            dir,
            *hidden,
            *cases,
            *seed,
            size.as_deref(),
            *jobs,
            case_log.as_deref(),
        ),
        Command::Measure {
            dir,
            repeat,
            out,
            size,
            seed,
        } => cmd::measure::run(cli.format, dir, *repeat, out.as_deref(), size, *seed),
        Command::Experiment {
            dir,
            repeat,
            out,
            size,
            seed,
            cases,
            jobs,
            case_log,
        } => cmd::experiment::run(
            cli.format,
            dir,
            *repeat,
            out.as_deref(),
            size,
            *seed,
            *cases,
            *jobs,
            case_log.as_deref(),
        ),
        Command::Matrix {
            dir,
            toolchains,
            opt_levels,
            build_systems,
            cmake_generators,
            repeat,
            size,
            seed,
            output_dir,
        } => cmd::matrix::run(
            cli.format,
            dir,
            toolchains,
            opt_levels,
            build_systems,
            cmake_generators,
            *repeat,
            size,
            *seed,
            output_dir.as_deref(),
        ),
        Command::Asm { dir } => cmd::asm::run(cli.format, dir),
        Command::Diff { a, b } => cmd::diff::run(cli.format, a, b),
        Command::Replay {
            capsule,
            dir,
            allow_drift,
        } => cmd::replay::run(cli.format, capsule, dir.as_deref(), *allow_drift),
        Command::Validate { path } => cmd::validate::run(cli.format, path),
    }
}
