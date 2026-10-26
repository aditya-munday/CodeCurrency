//! The engine: build, test, measure, and explain one solution.
//!
//! This is the module the CLI calls. It owns the order of operations and the
//! mapping from raw process outcomes to verdicts, so the verdict model is
//! defined in exactly one place.
//!
//! Everything that touches the host goes through `Docker`, which is a thin
//! wrapper over the `docker` binary. Keeping it in one type means the engine
//! can be tested against a fake when Docker is not available.

use crate::capsule::{Capsule, Metric, MetricKind, MetricMethod, Verdict};
use crate::explain::{self, Hint, Ruleset};
use crate::measure::CachegrindCounts;
use crate::toolchain::Toolchain;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("docker is not available: {0}")]
    NoDocker(String),
    #[error("build failed:\n{0}")]
    BuildFailed(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("capsule error: {0}")]
    Capsule(#[from] crate::capsule::CapsuleError),
    #[error("{0}")]
    Other(String),
}

/// Outcome of running one process under the runner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub exit_status: i32,
    pub signal: i32,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub peak_rss_bytes: u64,
    pub wall_ns: u64,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
    /// Captured stdout, capped by the policy.
    pub stdout: String,
    pub stderr: String,
    /// Allocation accounting, when the shim was linked in.
    pub allocations: Option<AllocationCounts>,
}

impl RunOutcome {
    /// Map a raw outcome onto the verdict model. This is the single definition
    /// of what "time limit", "memory limit", and so on mean.
    pub fn verdict(&self, memory_limit: u64) -> Verdict {
        if self.timed_out {
            return Verdict::TimeLimit;
        }
        if self.output_truncated {
            // Huge output is a runtime error, not a wrong answer: the solution
            // never got to produce a checkable result.
            return Verdict::RuntimeError;
        }
        // SIGKILL or SIGSEGV with high peak memory reads as an OOM kill.
        if self.signal == 9 && self.peak_rss_bytes >= memory_limit / 2 {
            return Verdict::MemoryLimit;
        }
        if self.signal == 11 && self.peak_rss_bytes >= memory_limit / 2 {
            return Verdict::MemoryLimit;
        }
        if self.signal != 0 {
            return Verdict::RuntimeError;
        }
        if self.exit_status != 0 {
            return Verdict::RuntimeError;
        }
        Verdict::Accepted
    }

    /// Parse the single JSON line the runner writes.
    pub fn from_runner_json(text: &str) -> Option<RunOutcome> {
        #[derive(Deserialize)]
        struct Raw {
            exit_status: i32,
            signal: i32,
            timed_out: bool,
            output_truncated: bool,
            peak_rss_bytes: u64,
            wall_ns: u64,
            stdout_bytes: u64,
            stderr_bytes: u64,
        }
        let raw: Raw = serde_json::from_str(text.trim()).ok()?;
        Some(RunOutcome {
            exit_status: raw.exit_status,
            signal: raw.signal,
            timed_out: raw.timed_out,
            output_truncated: raw.output_truncated,
            peak_rss_bytes: raw.peak_rss_bytes,
            wall_ns: raw.wall_ns,
            stdout_bytes: raw.stdout_bytes,
            stderr_bytes: raw.stderr_bytes,
            stdout: String::new(),
            stderr: String::new(),
            allocations: None,
        })
    }
}

/// Parse the runner's framed report.
///
/// The protocol is length-prefixed, so it is binary-safe and a submission
/// cannot forge it: the child's stdout is a pipe, and the framing is written to
/// the runner's own stdout, which the child never holds.
///
/// ```text
/// CCX-OUT <n>\n<n bytes>
/// CCX-ERR <m>\n<m bytes>
/// CCX-ALLOC <k>\n<k bytes>        (allocation-shim accounting, may be empty)
/// CCX-EMIT <p>\n<p bytes>         (a file carried out of the sandbox, may be empty)
/// CCX-VERDICT <json>\n
/// ```
///
/// Returns `None` if the framing is absent or inconsistent, which is itself a
/// signal that something is wrong rather than something to paper over.
pub fn parse_framed_report(raw: &[u8]) -> Option<RunOutcome> {
    parse_framed(raw).map(|(outcome, _)| outcome)
}

/// As `parse_framed_report`, but also returns the bytes of the emitted file.
///
/// Kept separate from `RunOutcome` so a compiled binary can never end up
/// serialised into a capsule as though it were part of the measurement.
pub fn parse_framed_report_with_emit(raw: &[u8]) -> Option<(RunOutcome, Option<Vec<u8>>)> {
    parse_framed(raw)
}

fn parse_framed(raw: &[u8]) -> Option<(RunOutcome, Option<Vec<u8>>)> {
    let mut pos = 0usize;

    fn read_line(raw: &[u8], pos: &mut usize) -> Option<String> {
        let start = *pos;
        let nl = raw[start..].iter().position(|b| *b == b'\n')? + start;
        let line = String::from_utf8_lossy(&raw[start..nl]).to_string();
        *pos = nl + 1;
        Some(line)
    }

    fn read_section(raw: &[u8], pos: &mut usize, tag: &str) -> Option<String> {
        let header = read_line(raw, pos)?;
        let len: usize = header.strip_prefix(tag)?.trim().parse().ok()?;
        if *pos + len > raw.len() {
            return None;
        }
        let body = String::from_utf8_lossy(&raw[*pos..*pos + len]).to_string();
        *pos += len;
        Some(body)
    }

    fn read_bytes(raw: &[u8], pos: &mut usize, tag: &str) -> Option<Vec<u8>> {
        let header = read_line(raw, pos)?;
        let len: usize = header.strip_prefix(tag)?.trim().parse().ok()?;
        if *pos + len > raw.len() {
            return None;
        }
        let body = raw[*pos..*pos + len].to_vec();
        *pos += len;
        Some(body)
    }

    let stdout = read_section(raw, &mut pos, "CCX-OUT ")?;
    let stderr = read_section(raw, &mut pos, "CCX-ERR ")?;
    let alloc_json = read_section(raw, &mut pos, "CCX-ALLOC ")?;
    let emitted = read_bytes(raw, &mut pos, "CCX-EMIT ")?;

    let verdict_line = read_line(raw, &mut pos)?;
    let json = verdict_line.strip_prefix("CCX-VERDICT ")?;
    let mut outcome = RunOutcome::from_runner_json(json)?;
    outcome.stdout = stdout;
    outcome.stderr = stderr;
    if !alloc_json.trim().is_empty() {
        outcome.allocations = AllocationCounts::from_json(&alloc_json);
    }
    Some((outcome, (!emitted.is_empty()).then_some(emitted)))
}

/// Everything the engine knows about one measurement attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measurement {
    pub outcome: RunOutcome,
    pub cachegrind: Option<CachegrindCounts>,
    pub allocations: Option<AllocationCounts>,
    pub binary_size: Option<u64>,
    pub hardware: Option<HardwareCounters>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllocationCounts {
    pub allocations: u64,
    pub allocated_bytes: u64,
    pub frees: u64,
    pub peak_live_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareCounters {
    pub instructions: u64,
    pub cache_misses: u64,
    pub branch_misses: u64,
}

impl AllocationCounts {
    pub fn from_json(text: &str) -> Option<Self> {
        serde_json::from_str(text.trim()).ok()
    }
}

impl Measurement {
    /// Flatten a measurement into capsule metrics, each carrying its honest
    /// label. Order is stable so capsules diff cleanly.
    pub fn to_metrics(&self) -> Vec<Metric> {
        let mut metrics = Vec::new();

        if let Some(c) = &self.cachegrind {
            metrics.extend(c.to_metrics());
            // Data references are needed for the cache-miss-rate hint, so they
            // travel with the capsule rather than being recomputed.
            metrics.push(Metric::count(
                "memory_refs",
                c.data_refs + c.instruction_refs,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ));
            metrics.push(Metric::count(
                "branches",
                c.branch_instructions,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ));
        }

        if let Some(a) = &self.allocations {
            metrics.push(Metric::count(
                "allocations",
                a.allocations,
                MetricKind::Measured,
                MetricMethod::Manual,
            ));
            metrics.push(Metric::bytes(
                "allocated_bytes",
                a.allocated_bytes,
                MetricKind::Measured,
                MetricMethod::Manual,
            ));
        }

        metrics.push(Metric::bytes(
            "peak_memory",
            self.outcome.peak_rss_bytes,
            MetricKind::Measured,
            MetricMethod::Getrusage,
        ));

        if let Some(size) = self.binary_size {
            metrics.push(Metric::bytes(
                "binary_size",
                size,
                MetricKind::Measured,
                MetricMethod::Stat,
            ));
        }

        if let Some(h) = &self.hardware {
            // A real hardware counter is recorded alongside the simulated one,
            // never instead of it: they answer different questions.
            metrics.push(Metric::count(
                "instructions_hw",
                h.instructions,
                MetricKind::Measured,
                MetricMethod::Perf,
            ));
        }

        metrics
    }

    pub fn hints(&self, rules: &Ruleset) -> Vec<Hint> {
        // Hints are computed from the same flattened metrics the capsule
        // carries, so a hint can always be reproduced from the capsule alone.
        let capsule = self.pseudo_capsule(rules);
        explain::explain(&capsule, rules)
    }

    fn pseudo_capsule(&self, rules: &Ruleset) -> Capsule {
        use crate::capsule::*;
        use std::collections::BTreeMap;
        Capsule {
            schema_version: CURRENT_SCHEMA_VERSION,
            capsule_id: None,
            created_at: String::new(),
            problem: ProblemRef {
                id: String::new(),
                statement_sha256: String::new(),
            },
            source: SourceRef {
                language: Language::Cpp,
                entry: String::new(),
                files: BTreeMap::new(),
                source_sha256: String::new(),
            },
            toolchain: Toolchain {
                name: ToolchainName::Gcc,
                version: String::new(),
                image: String::new(),
                image_digest: String::new(),
                libc: String::new(),
            },
            build: Build {
                argv: vec![],
                opt_level: String::new(),
                env: BTreeMap::new(),
                argv_program: vec![],
                stdin_sha256: None,
                stack_bytes: 0,
            },
            harness: Harness {
                version: HARNESS_VERSION.to_string(),
                metric_method: MetricMethod::Cachegrind,
                ruleset: rules.id(),
            },
            input: InputSpec {
                generator: String::new(),
                seed: 0,
                seed_set: SeedSet::Public,
                size: None,
            },
            host: Host {
                cpu_model: String::new(),
                arch: String::new(),
                os: String::new(),
                kernel: String::new(),
                nproc: 0,
                frequency_policy: String::new(),
                fingerprint: String::new(),
            },
            verdict: Verdict::Accepted,
            metrics: self.to_metrics(),
            repeatability: None,
        }
    }
}

/// A thin, testable wrapper over the `docker` binary.
#[derive(Debug, Clone)]
pub struct Docker {
    pub program: String,
}

impl Default for Docker {
    fn default() -> Self {
        Docker {
            program: "docker".to_string(),
        }
    }
}

impl Docker {
    pub fn available(&self) -> bool {
        Command::new(&self.program)
            .arg("version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Run `argv` and return (status, stdout, stderr) as text.
    ///
    /// The sandbox builds a full, self-describing argv beginning with the
    /// program name (`docker ...`), because a capsule records the whole command
    /// a result came from. A leading `docker` is therefore stripped here rather
    /// than added blindly, so the two conventions do not collide.
    pub fn run(&self, argv: &[String]) -> Result<(i32, String, String), EngineError> {
        let (status, out, err) = self.run_bytes(argv)?;
        Ok((
            status,
            String::from_utf8_lossy(&out).to_string(),
            String::from_utf8_lossy(&err).to_string(),
        ))
    }

    /// Run `argv` and return the raw stdout bytes.
    ///
    /// Used wherever stdout carries a framed artifact rather than text: the
    /// runner's `CCX-EMIT` frame can hold a compiled binary, and decoding that
    /// as UTF-8 would corrupt it and break the length prefix.
    pub fn run_bytes(&self, argv: &[String]) -> Result<(i32, Vec<u8>, Vec<u8>), EngineError> {
        let args: &[String] = match argv.first() {
            Some(first) if first == &self.program => &argv[1..],
            _ => argv,
        };
        let out = Command::new(&self.program)
            .args(args)
            .output()
            .map_err(|e| EngineError::NoDocker(e.to_string()))?;
        Ok((out.status.code().unwrap_or(-1), out.stdout, out.stderr))
    }

    /// The image reference for a toolchain, pinned by digest.
    pub fn image_ref(tc: &Toolchain) -> String {
        format!("{}@{}", tc.image, tc.image_digest)
    }

    /// Run a container to completion and copy `guest_path` out to `host_path`.
    ///
    /// This is how a build artifact leaves the sandbox without ever mounting a
    /// writable host directory: the container keeps a read-only source mount
    /// and tmpfs scratch, and exactly one file is copied across afterwards.
    /// The container is always removed, including on failure.
    pub fn run_and_extract(
        &self,
        argv: Vec<String>,
        container_name: &str,
        guest_path: &str,
        host_path: &Path,
    ) -> Result<(i32, String, String), EngineError> {
        let mut create_argv = argv;
        // `docker run` is replaced by `docker create` + `docker start` + `cp`
        // + `rm`, so the tmpfs holding the artifact still exists when we copy.
        if create_argv.len() >= 2 && create_argv[1] == "run" {
            create_argv[1] = "create".to_string();
        }

        let cleanup = || {
            let _ = Command::new(self.program.as_str())
                .args(["rm", "-f", container_name])
                .output();
        };

        let (status, stdout, stderr) = self.run(&create_argv)?;
        if status != 0 {
            cleanup();
            return Ok((status, stdout, stderr));
        }

        let (status, out2, err2) = self.run(&[
            "start".into(),
            "-a".into(),
            container_name.to_string(),
        ])?;

        if status == 0 {
            // Copy the artifact before removing the container: scratch is tmpfs
            // and dies with it.
            let _ = self.run(&[
                "cp".into(),
                format!("{container_name}:{guest_path}"),
                host_path.to_string_lossy().to_string(),
            ])?;
        }

        cleanup();
        Ok((status, format!("{stdout}{out2}"), format!("{stderr}{err2}")))
    }
}

/// Where the engine keeps its working state for one solution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub root: PathBuf,
    pub work_dir: PathBuf,
    pub out_dir: PathBuf,
}

impl Workspace {
    pub fn new(root: &Path) -> Self {
        Workspace {
            root: root.to_path_buf(),
            work_dir: root.join(".ccx"),
            out_dir: root.join(".ccx/out"),
        }
    }

    pub fn ensure(&self) -> Result<(), EngineError> {
        std::fs::create_dir_all(&self.out_dir)?;
        Ok(())
    }

    pub fn capsule_path(&self) -> PathBuf {
        self.root.join("capsule.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(signal: i32, exit: i32) -> RunOutcome {
        RunOutcome {
            exit_status: exit,
            signal,
            timed_out: false,
            output_truncated: false,
            peak_rss_bytes: 1024,
            wall_ns: 1,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout: String::new(),
            stderr: String::new(),
            allocations: None,
        }
    }

    #[test]
    fn timeout_maps_to_time_limit() {
        let mut o = outcome(9, 0);
        o.timed_out = true;
        assert_eq!(o.verdict(1 << 30), Verdict::TimeLimit);
    }

    #[test]
    fn huge_output_maps_to_runtime_error() {
        let mut o = outcome(9, 0);
        o.output_truncated = true;
        assert_eq!(o.verdict(1 << 30), Verdict::RuntimeError);
    }

    #[test]
    fn sigkill_with_high_memory_maps_to_memory_limit() {
        let mut o = outcome(9, 0);
        o.peak_rss_bytes = 900 * 1024 * 1024;
        assert_eq!(o.verdict(1024 * 1024 * 1024), Verdict::MemoryLimit);
    }

    #[test]
    fn sigsegv_with_low_memory_is_a_runtime_error() {
        let o = outcome(11, 0);
        assert_eq!(o.verdict(1 << 30), Verdict::RuntimeError);
    }

    #[test]
    fn clean_exit_is_accepted() {
        assert_eq!(outcome(0, 0).verdict(1 << 30), Verdict::Accepted);
    }

    #[test]
    fn nonzero_exit_is_a_runtime_error() {
        assert_eq!(outcome(0, 3).verdict(1 << 30), Verdict::RuntimeError);
    }

    #[test]
    fn runner_json_round_trips() {
        let line = r#"{"exit_status":0,"signal":0,"timed_out":false,"output_truncated":false,"peak_rss_bytes":1572864,"wall_ns":935500,"stdout_bytes":5,"stderr_bytes":0}"#;
        let o = RunOutcome::from_runner_json(line).unwrap();
        assert_eq!(o.peak_rss_bytes, 1_572_864);
        assert_eq!(o.wall_ns, 935_500);
    }

    #[test]
    fn framed_report_round_trips_with_binary_safe_output() {
        // Child output contains a newline and the literal word the framing uses,
        // to prove the length prefix is what delimits it, not a sentinel.
        let child_out = b"hello\nCCX-VERDICT {\"exit_status\":999}\n";
        let child_err = b"warn";
        let alloc = b"{\"allocations\":1000,\"allocated_bytes\":132096,\"frees\":1000,\"peak_live_bytes\":4096}";
        // The emitted section carries arbitrary bytes, including NUL and the
        // framing sentinel, to prove the artifact is delimited by its length.
        let emitted: Vec<u8> = vec![0x7f, b'E', b'L', b'F', 0, 0xff, b'\n', 0x01];
        let mut raw = Vec::new();
        for (tag, body) in [
            ("CCX-OUT ", child_out.as_slice()),
            ("CCX-ERR ", child_err.as_slice()),
            ("CCX-ALLOC ", alloc.as_slice()),
            ("CCX-EMIT ", emitted.as_slice()),
        ] {
            raw.extend_from_slice(format!("{tag}{}\n", body.len()).as_bytes());
            raw.extend_from_slice(body);
        }
        raw.extend_from_slice(
            b"CCX-VERDICT {\"exit_status\":0,\"signal\":0,\"timed_out\":false,\"output_truncated\":false,\"peak_rss_bytes\":100,\"wall_ns\":5,\"stdout_bytes\":1,\"stderr_bytes\":1}\n",
        );

        let (o, got_emitted) =
            parse_framed_report_with_emit(&raw).expect("framing parses");
        assert_eq!(got_emitted.as_deref(), Some(emitted.as_slice()));
        assert_eq!(o.exit_status, 0, "child output must not be able to forge the verdict");
        assert_eq!(o.stdout, String::from_utf8_lossy(child_out));
        assert_eq!(o.stderr, "warn");
        let a = o.allocations.expect("allocation accounting parsed");
        assert_eq!(a.allocations, 1000);
        assert_eq!(a.allocated_bytes, 132_096);
    }

    #[test]
    fn empty_alloc_section_is_not_an_error() {
        let raw = b"CCX-OUT 0\nCCX-ERR 0\nCCX-ALLOC 0\nCCX-EMIT 0\nCCX-VERDICT {\"exit_status\":0,\"signal\":0,\"timed_out\":false,\"output_truncated\":false,\"peak_rss_bytes\":1,\"wall_ns\":1,\"stdout_bytes\":0,\"stderr_bytes\":0}\n";
        let (o, emitted) = parse_framed_report_with_emit(raw).unwrap();
        assert!(o.allocations.is_none());
        assert!(emitted.is_none());
    }

    #[test]
    fn framed_report_rejects_missing_framing() {
        assert!(parse_framed_report(b"just some output\n").is_none());
    }

    #[test]
    fn framed_report_rejects_a_truncated_body() {
        let raw = b"CCX-OUT 100\nshort";
        assert!(parse_framed_report(raw).is_none());
    }

    #[test]
    fn signaled_child_is_never_a_clean_exit() {
        // The runner reports 128+signal for a signaled child; the engine must
        // read that as a runtime error, not success.
        let o = RunOutcome {
            exit_status: 139,
            signal: 11,
            timed_out: false,
            output_truncated: false,
            peak_rss_bytes: 1024,
            wall_ns: 1,
            stdout_bytes: 0,
            stderr_bytes: 0,
            stdout: String::new(),
            stderr: String::new(),
            allocations: None,
        };
        assert_ne!(o.verdict(1 << 30), Verdict::Accepted);
        assert_eq!(o.verdict(1 << 30), Verdict::RuntimeError);
    }

    #[test]
    fn metrics_never_label_indicative_without_uncertainty() {
        let m = Measurement {
            outcome: outcome(0, 0),
            cachegrind: None,
            allocations: None,
            binary_size: Some(1234),
            hardware: None,
        };
        for metric in m.to_metrics() {
            if metric.kind == MetricKind::Indicative {
                assert!(metric.uncertainty.is_some(), "{:?}", metric.name);
            }
        }
    }

    #[test]
    fn simulated_and_measured_instruction_counts_coexist() {
        let m = Measurement {
            outcome: outcome(0, 0),
            cachegrind: Some(CachegrindCounts {
                instruction_refs: 1000,
                ..Default::default()
            }),
            allocations: None,
            binary_size: None,
            hardware: Some(HardwareCounters {
                instructions: 1010,
                cache_misses: 5,
                branch_misses: 2,
            }),
        };
        let metrics = m.to_metrics();
        let sim = metrics.iter().find(|x| x.name == "instructions").unwrap();
        let hw = metrics.iter().find(|x| x.name == "instructions_hw").unwrap();
        assert_eq!(sim.kind, MetricKind::Simulated);
        assert_eq!(hw.kind, MetricKind::Measured);
    }

    #[test]
    fn image_ref_is_digest_pinned() {
        let tc = Toolchain {
            name: crate::capsule::ToolchainName::Gcc,
            version: "14.2.0".into(),
            image: "ccx-gcc".into(),
            image_digest: "sha256:abc".into(),
            libc: "glibc 2.41".into(),
            compiler: "gcc".into(),
        };
        assert_eq!(Docker::image_ref(&tc), "ccx-gcc@sha256:abc");
    }
}
