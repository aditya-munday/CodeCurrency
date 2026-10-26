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
    /// Create a new solution scaffold in the current directory.
    Init {
        /// Problem id to target, e.g. `top-k/heap-vs-sort`.
        #[arg(long)]
        problem: Option<String>,
        /// Language for the scaffold.
        #[arg(long, default_value = "cpp")]
        language: String,
        /// Directory to create the scaffold in.
        #[arg(long, default_value = ".")]
        dir: PathBuf,
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
    },
    /// Measure deterministic metrics and emit a capsule.
    Measure {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        /// Repeat the measurement N times to record the spread.
        #[arg(long, default_value_t = 1)]
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
        Command::Init {
            problem,
            language,
            dir,
        } => cmd::init::run(cli.format, problem.as_deref(), language, dir),
        Command::Build { dir } => cmd::build::run(cli.format, dir),
        Command::Run {
            dir,
            size,
            seed,
            show_expected,
        } => cmd::run::run(cli.format, dir, size, *seed, *show_expected),
        Command::Test { dir, hidden } => cmd::test::run(cli.format, dir, *hidden),
        Command::Measure {
            dir,
            repeat,
            out,
            size,
            seed,
        } => cmd::measure::run(cli.format, dir, *repeat, out.as_deref(), size, *seed),
        Command::Asm { dir } => cmd::asm::run(cli.format, dir),
        Command::Diff { a, b } => cmd::diff::run(cli.format, a, b),
        Command::Replay {
            capsule,
            allow_drift,
        } => cmd::replay::run(cli.format, capsule, *allow_drift),
        Command::Validate { path } => cmd::validate::run(cli.format, path),
    }
}
