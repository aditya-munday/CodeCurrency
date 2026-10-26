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
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// Measurement harness version, bumped when measurement behaviour changes.
pub const HARNESS_VERSION: &str = "0.1.0";

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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UncertaintyKind {
    Stdev,
    Ci95,
    Range,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Uncertainty {
    pub kind: UncertaintyKind,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    C,
    Cpp,
    #[serde(rename = "asm-x86-64")]
    AsmX86_64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolchainName {
    Gcc,
    Clang,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeedSet {
    Public,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProblemRef {
    pub id: String,
    pub statement_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceRef {
    pub language: Language,
    pub entry: String,
    /// relative path -> "sha256:<hex>"
    pub files: BTreeMap<String, String>,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Toolchain {
    pub name: ToolchainName,
    pub version: String,
    pub image: String,
    pub image_digest: String,
    pub libc: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Build {
    pub argv: Vec<String>,
    pub opt_level: String,
    pub env: BTreeMap<String, String>,
    pub argv_program: Vec<String>,
    pub stdin_sha256: Option<String>,
    pub stack_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Harness {
    pub version: String,
    pub metric_method: MetricMethod,
    pub ruleset: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputSpec {
    pub generator: String,
    pub seed: u64,
    pub seed_set: SeedSet,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
pub struct Repeatability {
    pub runs: u32,
    pub per_metric: BTreeMap<String, Spread>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Spread {
    pub min: f64,
    pub max: f64,
    pub median: f64,
    pub spread_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capsule {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule_id: Option<String>,
    pub created_at: String,
    pub problem: ProblemRef,
    pub source: SourceRef,
    pub toolchain: Toolchain,
    pub build: Build,
    pub harness: Harness,
    pub input: InputSpec,
    pub host: Host,
    pub verdict: Verdict,
    pub metrics: Vec<Metric>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeatability: Option<Repeatability>,
}

impl Capsule {
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
            if !hash.starts_with("sha256:") || hash.len() != 7 + 64 {
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
        if !self.toolchain.image_digest.starts_with("sha256:") {
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
        }

        if let Some(id) = &self.capsule_id {
            let computed = self.compute_id()?;
            if &computed != id {
                return Err(CapsuleError::IdMismatch {
                    claimed: id.clone(),
                    computed,
                });
            }
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
        let capsule = if version as u32 == CURRENT_SCHEMA_VERSION {
            serde_json::from_value(value)?
        } else {
            let migrated = migrate::migrate_value(value, version as u32)?;
            serde_json::from_value(migrated)?
        };
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
