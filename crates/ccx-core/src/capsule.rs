//! Experiment capsules: the unit of evidence.
//!
//! A capsule is a versioned JSON document describing one measured run in
//! enough detail to reproduce it. See `CAPSULE_SPEC.md` for the contract this
//! module implements.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;

pub mod migrate;
pub mod repeatability;

/// The schema version this build writes.
pub const CURRENT_SCHEMA_VERSION: u32 = 14;
pub const LEGACY_BINARY_DIGEST_UNAVAILABLE: &str = "unavailable_legacy";
pub const DEFAULT_BUILD_REPEAT: u32 = 3;

/// Measurement harness version, bumped when measurement behaviour changes.
pub const HARNESS_VERSION: &str = "0.18.0";

/// Canonical environment for both compilation and execution. Every other
/// variable is dropped so counts cannot drift between runs.
pub const FIXED_ENV: &[(&str, &str)] = &[
    (
        "PATH",
        "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("LANG", "C"),
    ("LC_ALL", "C"),
    ("TZ", "UTC"),
    ("SOURCE_DATE_EPOCH", "0"),
];

pub fn utc_timestamp_now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    format_utc_timestamp(seconds)
}

fn format_utc_timestamp(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[derive(Debug, thiserror::Error)]
pub enum CapsuleError {
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported schema_version {found}; this build understands up to {supported}")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("capsule failed validation: {0}")]
    Invalid(String),
    #[error("capsule_id mismatch: document says {claimed}, content hashes to {computed}")]
    IdMismatch { claimed: String, computed: String },
}

/// How a number was obtained. This label is mandatory on every metric and is
/// what keeps the platform honest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    /// A real hardware or OS counter.
    Measured,
    /// A deterministic model (e.g. cachegrind). Identical on any machine.
    Simulated,
    /// Wall-clock. Machine-dependent, never used for ranking.
    Indicative,
}

impl fmt::Display for MetricKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            MetricKind::Measured => "measured",
            MetricKind::Simulated => "simulated",
            MetricKind::Indicative => "indicative",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricMethod {
    Cachegrind,
    Perf,
    Getrusage,
    Stat,
    Manual,
    ValgrindMemcheck,
    WallClock,
    Runner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    Stdev,
    Ci95,
    Range,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uncertainty {
    pub kind: UncertaintyKind,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Metric {
    pub name: String,
    pub value: f64,
    pub unit: String,
    pub kind: MetricKind,
    pub method: MetricMethod,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<Uncertainty>,
}

impl Metric {
    pub fn count(name: &str, value: u64, kind: MetricKind, method: MetricMethod) -> Self {
        Metric {
            name: name.to_string(),
            value: value as f64,
            unit: "count".to_string(),
            kind,
            method,
            uncertainty: None,
        }
    }

    pub fn bytes(name: &str, value: u64, kind: MetricKind, method: MetricMethod) -> Self {
        Metric {
            name: name.to_string(),
            value: value as f64,
            unit: "bytes".to_string(),
            kind,
            method,
            uncertainty: None,
        }
    }

    pub fn machine_dependent(&self) -> bool {
        self.kind == MetricKind::Indicative
            || matches!(
                self.method,
                MetricMethod::Getrusage | MetricMethod::Perf | MetricMethod::WallClock
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    C,
    Cpp,
    Rust,
    D,
    Zig,
    Odin,
    Go,
    #[serde(rename = "asm-x86-64")]
    AsmX86_64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolchainName {
    Gcc,
    #[serde(rename = "gcc-13")]
    Gcc13,
    Clang,
    #[serde(rename = "clang-18")]
    Clang18,
    Rust,
    Gdc,
    Zig,
    Odin,
    Go,
}

impl ToolchainName {
    pub fn profile_id(self) -> &'static str {
        match self {
            Self::Gcc => "gcc",
            Self::Gcc13 => "gcc-13",
            Self::Clang => "clang",
            Self::Clang18 => "clang-18",
            Self::Rust => "rust",
            Self::Gdc => "gdc",
            Self::Zig => "zig",
            Self::Odin => "odin",
            Self::Go => "go",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSet {
    Public,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProblemRef {
    pub id: String,
    pub statement_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRef {
    pub language: Language,
    pub entry: String,
    /// relative path -> "sha256:<hex>"
    pub files: BTreeMap<String, String>,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchain {
    pub name: ToolchainName,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_fingerprint: Option<String>,
    pub libc: String,
}

impl Toolchain {
    pub fn same_environment(&self, other: &Self) -> bool {
        match (
            self.environment_fingerprint.as_deref(),
            other.environment_fingerprint.as_deref(),
        ) {
            (Some(left), Some(right)) => left == right,
            (None, None) => self.image_digest == other.image_digest,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Build {
    pub argv: Vec<String>,
    pub opt_level: String,
    pub binary_sha256: String,
    pub build_repeat: u32,
    pub env: BTreeMap<String, String>,
    pub argv_program: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub argv_runner: Vec<String>,
    pub stdin_sha256: Option<String>,
    pub stack_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceBuild {
    pub binary_sha256: String,
    pub argv: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    pub version: String,
    pub metric_method: MetricMethod,
    pub ruleset: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    pub generator: String,
    pub seed: u64,
    pub seed_set: SeedSet,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub cpu_model: String,
    pub arch: String,
    pub os: String,
    pub kernel: String,
    pub nproc: u32,
    pub frequency_policy: String,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accepted,
    WrongAnswer,
    TimeLimit,
    MemoryLimit,
    RuntimeError,
    BuildError,
    RejectedUnsafe,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Verdict::Accepted => "accepted",
            Verdict::WrongAnswer => "wrong answer",
            Verdict::TimeLimit => "time limit",
            Verdict::MemoryLimit => "memory limit",
            Verdict::RuntimeError => "runtime error",
            Verdict::BuildError => "build error",
            Verdict::RejectedUnsafe => "rejected unsafe",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repeatability {
    pub runs: u32,
    pub per_metric: BTreeMap<String, Spread>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spread {
    #[serde(default)]
    pub samples: u32,
    pub min: f64,
    pub max: f64,
    pub median: f64,
    pub spread_pct: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p95: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p99: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capsule {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule_id: Option<String>,
    pub created_at: String,
    pub problem: ProblemRef,
    pub source: SourceRef,
    pub toolchain: Toolchain,
    pub build: Build,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_build: Option<ReferenceBuild>,
    pub harness: Harness,
    pub input: InputSpec,
    pub host: Host,
    pub verdict: Verdict,
    pub metrics: Vec<Metric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeatability: Option<Repeatability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_repeatability: Option<Repeatability>,
}

impl Capsule {
    pub fn same_problem_definition(&self, other: &Self) -> bool {
        self.problem.id == other.problem.id
            && self.problem.statement_sha256 == other.problem.statement_sha256
            && self
                .source
                .files
                .iter()
                .filter(|(path, _)| path.starts_with("problem/"))
                .eq(other
                    .source
                    .files
                    .iter()
                    .filter(|(path, _)| path.starts_with("problem/")))
    }

    pub fn same_comparison_context(&self, other: &Self) -> bool {
        self.same_problem_definition(other)
            && self.source.language == other.source.language
            && self.toolchain.name == other.toolchain.name
            && self.toolchain.version == other.toolchain.version
            && self.toolchain.same_environment(&other.toolchain)
            && self.harness.version == other.harness.version
            && self.build.stdin_sha256.is_some()
            && self.build.stdin_sha256 == other.build.stdin_sha256
            && self.build.argv_program == other.build.argv_program
            && self.build.argv_runner == other.build.argv_runner
            && self.build.env == other.build.env
            && self.build.stack_bytes == other.build.stack_bytes
            && self.host.arch == other.host.arch
            && self.host.os == other.host.os
    }

    /// Canonical JSON of the body with `capsule_id` removed and keys sorted.
    /// `serde_json::Value` maps are `BTreeMap`-backed, so serialisation is
    /// already key-sorted, which makes this stable across builds.
    pub fn canonical_body(&self) -> Result<String, CapsuleError> {
        let mut value = serde_json::to_value(self)?;
        if let serde_json::Value::Object(map) = &mut value {
            map.remove("capsule_id");
        }
        Ok(serde_json::to_string(&value)?)
    }

    pub fn compute_id(&self) -> Result<String, CapsuleError> {
        Ok(sha256_hex(self.canonical_body()?.as_bytes()))
    }

    /// Fill in `capsule_id` from the current content.
    pub fn seal(&mut self) -> Result<(), CapsuleError> {
        let id = self.compute_id()?;
        self.capsule_id = Some(id);
        Ok(())
    }

    /// Full validation: schema version, structural rules, and identity.
    pub fn validate(&self) -> Result<(), CapsuleError> {
        if self.schema_version != CURRENT_SCHEMA_VERSION {
            return Err(CapsuleError::UnsupportedVersion {
                found: self.schema_version,
                supported: CURRENT_SCHEMA_VERSION,
            });
        }

        if self.source.files.is_empty() {
            return Err(CapsuleError::Invalid("source.files is empty".into()));
        }
        if !self.source.files.contains_key(&self.source.entry) {
            return Err(CapsuleError::Invalid(format!(
                "source.entry {:?} is not present in source.files",
                self.source.entry
            )));
        }
        for (path, hash) in &self.source.files {
            if path.is_empty()
                || std::path::Path::new(path)
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
            {
                return Err(CapsuleError::Invalid(format!(
                    "source file path {path:?} must be a normalized relative path"
                )));
            }
            let digest = hash.strip_prefix("sha256:").unwrap_or("");
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(CapsuleError::Invalid(format!(
                    "source.files[{path:?}] must be \"sha256:<64 hex>\""
                )));
            }
        }

        let expected_source = hash_source_files(&self.source.files);
        if expected_source != self.source.source_sha256 {
            return Err(CapsuleError::Invalid(format!(
                "source.source_sha256 mismatch: document says {}, recomputed {}",
                self.source.source_sha256, expected_source
            )));
        }

        // A digest is a contract; a tag is not.
        let image_digest = self
            .toolchain
            .image_digest
            .strip_prefix("sha256:")
            .unwrap_or("");
        if image_digest.len() != 64 || !image_digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CapsuleError::Invalid(
                "toolchain.image_digest must be \"sha256:<hex>\"; tags like :latest are not allowed"
                    .into(),
            ));
        }
        if self.toolchain.image.contains(":latest") {
            return Err(CapsuleError::Invalid(
                "toolchain.image must be pinned by digest, not :latest".into(),
            ));
        }
        if self.build.binary_sha256 != LEGACY_BINARY_DIGEST_UNAVAILABLE {
            let binary_sha256 = &self.build.binary_sha256;
            let digest = binary_sha256.strip_prefix("sha256:").unwrap_or("");
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(CapsuleError::Invalid(
                    "build.binary_sha256 must be \"sha256:<64 hex>\"".into(),
                ));
            }
        }
        if self.build.build_repeat == 0 || self.build.build_repeat > 20 {
            return Err(CapsuleError::Invalid(
                "build.build_repeat must be between 1 and 20".into(),
            ));
        }
        if let Some(fingerprint) = &self.toolchain.environment_fingerprint {
            let digest = fingerprint.strip_prefix("sha256:").unwrap_or("");
            if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(CapsuleError::Invalid(
                    "toolchain.environment_fingerprint must be \"sha256:<64 hex>\"".into(),
                ));
            }
        }

        let expected_host = host_fingerprint(
            &self.host.cpu_model,
            &self.host.arch,
            &self.host.os,
            &self.host.kernel,
            self.host.nproc,
        );
        if expected_host != self.host.fingerprint {
            return Err(CapsuleError::Invalid(format!(
                "host.fingerprint mismatch: document says {}, recomputed {}",
                self.host.fingerprint, expected_host
            )));
        }

        for m in &self.metrics {
            if m.name.trim().is_empty() {
                return Err(CapsuleError::Invalid("metric with empty name".into()));
            }
            if !m.value.is_finite() {
                return Err(CapsuleError::Invalid(format!(
                    "metric {:?} has non-finite value",
                    m.name
                )));
            }
            if m.kind == MetricKind::Indicative && m.uncertainty.is_none() {
                // Indicative numbers without a spread are exactly the "fake
                // precision" the platform promises not to ship.
                return Err(CapsuleError::Invalid(format!(
                    "indicative metric {:?} must carry uncertainty",
                    m.name
                )));
            }
            if let Some(uncertainty) = &m.uncertainty {
                if !uncertainty.value.is_finite() || uncertainty.value < 0.0 {
                    return Err(CapsuleError::Invalid(format!(
                        "metric {:?} has invalid uncertainty",
                        m.name
                    )));
                }
            }
        }

        let reference_wall_clock = self
            .metrics
            .iter()
            .find(|metric| metric.name == "reference_wall_clock");
        let paired_speedup_ratio = self
            .metrics
            .iter()
            .find(|metric| metric.name == "paired_speedup_ratio");
        if reference_wall_clock.is_some() != paired_speedup_ratio.is_some() {
            return Err(CapsuleError::Invalid(
                "reference_wall_clock and paired_speedup_ratio must be recorded together".into(),
            ));
        }
        if let (Some(reference), Some(ratio)) = (reference_wall_clock, paired_speedup_ratio) {
            if reference.kind != MetricKind::Indicative
                || reference.method != MetricMethod::WallClock
                || reference.unit != "nanoseconds"
                || ratio.kind != MetricKind::Indicative
                || ratio.method != MetricMethod::WallClock
                || ratio.unit != "ratio"
                || ratio.value <= 0.0
            {
                return Err(CapsuleError::Invalid(
                    "paired reference metrics have invalid labels, units, or values".into(),
                ));
            }
            match &self.reference_build {
                Some(reference_build) => {
                    let digest = reference_build
                        .binary_sha256
                        .strip_prefix("sha256:")
                        .unwrap_or("");
                    if digest.len() != 64
                        || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                        || reference_build.argv.is_empty()
                        || reference_build
                            .argv
                            .iter()
                            .any(|argument| argument.contains('\0'))
                        || !self.source.files.contains_key("problem/reference.cpp")
                    {
                        return Err(CapsuleError::Invalid(
                            "paired reference build metadata is incomplete or invalid".into(),
                        ));
                    }
                }
                None if self.harness.version == "0.12.0" => {}
                None => {
                    return Err(CapsuleError::Invalid(
                        "paired metrics require reference build metadata".into(),
                    ))
                }
            }
        } else if self.reference_build.is_some() {
            return Err(CapsuleError::Invalid(
                "reference build metadata requires paired timing metrics".into(),
            ));
        }

        if let Some(repeatability) = &self.repeatability {
            if repeatability.runs < 2 {
                return Err(CapsuleError::Invalid(
                    "repeatability.runs must be at least 2".into(),
                ));
            }
            if repeatability.runs > 100 {
                return Err(CapsuleError::Invalid(
                    "repeatability.runs must not exceed 100".into(),
                ));
            }
            if self.source.language == Language::Go && repeatability.runs < 20 {
                return Err(CapsuleError::Invalid(
                    "Go capsules require at least 20 repeatability runs".into(),
                ));
            }
            for (name, spread) in &repeatability.per_metric {
                if spread.samples != repeatability.runs
                    || spread.samples > 100
                    || !spread.min.is_finite()
                    || !spread.max.is_finite()
                    || !spread.median.is_finite()
                    || !spread.spread_pct.is_finite()
                    || spread.min > spread.median
                    || spread.median > spread.max
                    || spread.spread_pct < 0.0
                    || spread.p95.is_some_and(|value| {
                        !value.is_finite() || value < spread.median || value > spread.max
                    })
                    || spread.p99.is_some_and(|value| {
                        !value.is_finite() || value < spread.median || value > spread.max
                    })
                    || matches!((spread.p95, spread.p99), (Some(p95), Some(p99)) if p99 < p95)
                {
                    return Err(CapsuleError::Invalid(format!(
                        "repeatability metric {name:?} has invalid spread"
                    )));
                }
            }
        }

        if self.build.build_repeat == 0 || self.build.build_repeat > 20 {
            return Err(CapsuleError::Invalid(
                "build_repeat must be between 1 and 20".into(),
            ));
        }

        match (&self.build_repeatability, self.build.build_repeat) {
            (Some(repeatability), runs) => {
                if runs < 2 {
                    return Err(CapsuleError::Invalid(
                        "build_repeatability requires at least two builds".into(),
                    ));
                }
                if repeatability.runs != runs {
                    return Err(CapsuleError::Invalid(
                        "build_repeatability.runs must equal build.build_repeat".into(),
                    ));
                }
                for name in repeatability.per_metric.keys() {
                    if !matches!(
                        name.as_str(),
                        "build_wall_clock" | "build_user_cpu_time" | "build_system_cpu_time"
                    ) {
                        return Err(CapsuleError::Invalid(format!(
                            "unknown build repeatability metric {name:?}"
                        )));
                    }
                    if !self.metrics.iter().any(|metric| metric.name == *name) {
                        return Err(CapsuleError::Invalid(format!(
                            "build repeatability metric {name:?} has no corresponding metric"
                        )));
                    }
                }
                for (name, spread) in &repeatability.per_metric {
                    if spread.samples != runs
                        || !spread.min.is_finite()
                        || !spread.max.is_finite()
                        || !spread.median.is_finite()
                        || !spread.spread_pct.is_finite()
                        || spread.min > spread.median
                        || spread.median > spread.max
                        || spread.spread_pct < 0.0
                        || spread.p95.is_some_and(|value| {
                            !value.is_finite() || value < spread.median || value > spread.max
                        })
                        || spread.p99.is_some_and(|value| {
                            !value.is_finite() || value < spread.median || value > spread.max
                        })
                        || matches!((spread.p95, spread.p99), (Some(p95), Some(p99)) if p99 < p95)
                    {
                        return Err(CapsuleError::Invalid(format!(
                            "build repeatability metric {name:?} has invalid spread"
                        )));
                    }
                    let metric = self
                        .metrics
                        .iter()
                        .find(|metric| metric.name == *name)
                        .unwrap();
                    if metric.value != spread.median
                        || metric.uncertainty.as_ref().is_none_or(|uncertainty| {
                            uncertainty.kind != UncertaintyKind::Ci95
                                || !uncertainty.value.is_finite()
                                || uncertainty.value < 0.0
                        })
                    {
                        return Err(CapsuleError::Invalid(format!(
                            "build metric {name:?} does not match its repeatability summary"
                        )));
                    }
                }
                if !repeatability.per_metric.contains_key("build_wall_clock") {
                    return Err(CapsuleError::Invalid(
                        "build repeatability must include build_wall_clock".into(),
                    ));
                }
            }
            (None, 1) => {}
            (None, _) => {
                return Err(CapsuleError::Invalid(
                    "repeated builds require build_repeatability data".into(),
                ))
            }
        }

        let id = self
            .capsule_id
            .as_ref()
            .ok_or_else(|| CapsuleError::Invalid("capsule_id is missing".into()))?;
        if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CapsuleError::Invalid(
                "capsule_id must be a 64-character SHA-256 digest".into(),
            ));
        }
        let computed = self.compute_id()?;
        if &computed != id {
            return Err(CapsuleError::IdMismatch {
                claimed: id.clone(),
                computed,
            });
        }

        Ok(())
    }

    pub fn to_json_pretty(&self) -> Result<String, CapsuleError> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    pub fn from_json(text: &str) -> Result<Self, CapsuleError> {
        let value: serde_json::Value = serde_json::from_str(text)?;
        let version = value
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| CapsuleError::Invalid("missing schema_version".into()))?;
        let version = u32::try_from(version)
            .map_err(|_| CapsuleError::Invalid("schema_version is out of range".into()))?;
        let migrated = version != CURRENT_SCHEMA_VERSION;
        let mut capsule: Capsule = if !migrated {
            serde_json::from_value(value)?
        } else {
            let migrated = migrate::migrate_value(value, version)?;
            serde_json::from_value(migrated)?
        };
        if migrated {
            capsule.capsule_id = None;
            capsule.seal()?;
        }
        capsule.validate()?;
        Ok(capsule)
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// `source_sha256`: over `"<path>\0<hex>\n"` for each entry, paths sorted.
pub fn hash_source_files(files: &BTreeMap<String, String>) -> String {
    let mut buf = String::new();
    for (path, hash) in files {
        let hex = hash.strip_prefix("sha256:").unwrap_or(hash);
        buf.push_str(path);
        buf.push('\0');
        buf.push_str(hex);
        buf.push('\n');
    }
    sha256_hex(buf.as_bytes())
}

pub fn host_fingerprint(cpu_model: &str, arch: &str, os: &str, kernel: &str, nproc: u32) -> String {
    sha256_hex(format!("{cpu_model}|{arch}|{os}|{kernel}|{nproc}").as_bytes())
}

pub fn fixed_env_map() -> BTreeMap<String, String> {
    FIXED_ENV
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn getrusage_metrics_are_machine_dependent_even_when_measured() {
        let counter = Metric::count(
            "user_cpu_time",
            10,
            MetricKind::Measured,
            MetricMethod::Getrusage,
        );
        let deterministic = Metric::count(
            "allocations",
            10,
            MetricKind::Measured,
            MetricMethod::ValgrindMemcheck,
        );
        let hardware = Metric::count(
            "instructions_hw",
            10,
            MetricKind::Measured,
            MetricMethod::Perf,
        );
        assert!(counter.machine_dependent());
        assert!(hardware.machine_dependent());
        assert!(!deterministic.machine_dependent());
    }

    #[test]
    fn wall_clock_metrics_are_machine_dependent_even_when_measured() {
        let build_time = Metric::count(
            "build_wall_clock",
            10,
            MetricKind::Measured,
            MetricMethod::WallClock,
        );
        assert!(build_time.machine_dependent());
    }

    #[test]
    fn golden_capsule_round_trips_with_its_identity() {
        let capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v2.json")).unwrap();
        assert_eq!(capsule.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            capsule.capsule_id.as_deref(),
            Some(capsule.compute_id().unwrap().as_str())
        );
        assert!(capsule
            .metrics
            .iter()
            .any(|metric| metric.name == "wall_clock"));
    }

    #[test]
    fn schema_eleven_repeatability_capsule_round_trips_with_tail_quantiles() {
        let json = include_str!("../tests/fixtures/capsule-v11.json");
        let capsule = Capsule::from_json(json).unwrap();
        assert_eq!(capsule.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(
            capsule.build.binary_sha256,
            LEGACY_BINARY_DIGEST_UNAVAILABLE
        );
        assert_eq!(capsule.repeatability.as_ref().unwrap().runs, 20);
        let instructions = &capsule.repeatability.as_ref().unwrap().per_metric["instructions"];
        assert_eq!(instructions.min, instructions.max);
        assert_eq!(instructions.p95, Some(instructions.median));
        assert_eq!(instructions.p99, Some(instructions.median));
        let encoded = capsule.to_json_pretty().unwrap();
        let decoded = Capsule::from_json(&encoded).unwrap();
        assert_eq!(decoded, capsule);
    }

    #[test]
    fn schema_twelve_capsule_round_trips_with_executable_digest() {
        let json = include_str!("../tests/fixtures/capsule-v12.json");
        let capsule = Capsule::from_json(json).unwrap();
        assert_eq!(capsule.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(capsule.build.binary_sha256.starts_with("sha256:"));
        let encoded = capsule.to_json_pretty().unwrap();
        let decoded = Capsule::from_json(&encoded).unwrap();
        assert_eq!(decoded, capsule);
    }

    #[test]
    fn schema_thirteen_paired_capsule_round_trips_reference_provenance() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        capsule.schema_version = CURRENT_SCHEMA_VERSION;
        capsule.build.build_repeat = 1;
        capsule.metrics.push(Metric {
            name: "reference_wall_clock".into(),
            value: 2_000_000.0,
            unit: "nanoseconds".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 10_000.0,
            }),
        });
        capsule.metrics.push(Metric {
            name: "paired_speedup_ratio".into(),
            value: 2.0,
            unit: "ratio".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 0.02,
            }),
        });
        capsule.reference_build = Some(ReferenceBuild {
            binary_sha256: format!("sha256:{}", "b".repeat(64)),
            argv: vec![
                "g++".into(),
                "-O2".into(),
                "-std=c++20".into(),
                "/src0/reference.cpp".into(),
            ],
        });
        capsule.seal().unwrap();
        assert!(capsule.validate().is_ok());
        let encoded = capsule.to_json_pretty().unwrap();
        let decoded = Capsule::from_json(&encoded).unwrap();
        assert_eq!(decoded, capsule);
    }

    #[test]
    fn schema_twelve_requires_the_binary_digest_field() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        value["build"]
            .as_object_mut()
            .unwrap()
            .remove("binary_sha256");
        assert!(matches!(
            Capsule::from_json(&value.to_string()),
            Err(CapsuleError::Json(_))
        ));
    }

    #[test]
    fn paired_wall_clock_metrics_require_a_complete_indicative_pair() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        capsule.harness.version = "0.12.0".into();
        capsule.metrics.push(Metric {
            name: "reference_wall_clock".into(),
            value: 2_000_000.0,
            unit: "nanoseconds".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 10_000.0,
            }),
        });
        capsule.metrics.push(Metric {
            name: "paired_speedup_ratio".into(),
            value: 2.0,
            unit: "ratio".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 0.02,
            }),
        });
        capsule.seal().unwrap();
        assert!(capsule.validate().is_ok());

        capsule.metrics.pop();
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn paired_speedup_ratio_rejects_invalid_labels_and_values() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        capsule.metrics.push(Metric {
            name: "reference_wall_clock".into(),
            value: 2_000_000.0,
            unit: "nanoseconds".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 10_000.0,
            }),
        });
        capsule.metrics.push(Metric {
            name: "paired_speedup_ratio".into(),
            value: 0.0,
            unit: "ratio".into(),
            kind: MetricKind::Indicative,
            method: MetricMethod::WallClock,
            uncertainty: Some(Uncertainty {
                kind: UncertaintyKind::Ci95,
                value: 0.02,
            }),
        });
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn binary_digest_must_be_a_sha256_digest() {
        let json = include_str!("../tests/fixtures/capsule-v11.json");
        let mut capsule = Capsule::from_json(json).unwrap();
        capsule.build.binary_sha256 = format!("sha256:{}", "a".repeat(64));
        capsule.seal().unwrap();
        assert!(capsule.validate().is_ok());
        capsule.build.binary_sha256 = "sha256:invalid".into();
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn repeatability_sample_count_must_match_run_count() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        let repeatability = capsule.repeatability.as_mut().unwrap();
        repeatability
            .per_metric
            .values_mut()
            .next()
            .unwrap()
            .samples -= 1;
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn repeated_build_capsules_require_a_matching_summary() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        capsule.build.build_repeat = 3;
        capsule.build_repeatability = Some(Repeatability {
            runs: 2,
            per_metric: Default::default(),
        });
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn capsule_cannot_request_more_than_twenty_clean_builds() {
        let mut capsule =
            Capsule::from_json(include_str!("../tests/fixtures/capsule-v12.json")).unwrap();
        capsule.build.build_repeat = 21;
        capsule.seal().unwrap();
        assert!(matches!(capsule.validate(), Err(CapsuleError::Invalid(_))));
    }

    #[test]
    fn go_capsule_v7_round_trips_with_its_identity() {
        let json = include_str!("../tests/fixtures/go-capsule-v7.json");
        let capsule = Capsule::from_json(json).unwrap();
        assert_eq!(capsule.schema_version, CURRENT_SCHEMA_VERSION);
        assert_eq!(capsule.source.language, Language::Go);
        assert_eq!(capsule.repeatability.as_ref().unwrap().runs, 20);
        let encoded = capsule.to_json_pretty().unwrap();
        let decoded = Capsule::from_json(&encoded).unwrap();
        assert_eq!(decoded, capsule);
    }

    #[test]
    fn unknown_nested_capsule_fields_are_rejected() {
        let mut value: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/capsule-v2.json")).unwrap();
        value["toolchain"]["unrecognized"] = serde_json::json!("value");
        let result = Capsule::from_json(&value.to_string());
        assert!(matches!(result, Err(CapsuleError::Json(_))));
    }

    #[test]
    fn timestamp_formats_unix_epoch_as_utc() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01T00:00:00Z");
    }
}
