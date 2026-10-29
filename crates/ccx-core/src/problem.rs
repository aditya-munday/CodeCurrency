//! The problem format.
//!
//! Layer 5 of Phase 1: one folder per problem, containing everything needed to
//! state, test, check, explain, and reference-solve it. `validate` walks a
//! folder end to end and refuses to accept a problem that is not internally
//! consistent.
//!
//! ```text
//! problems/<category>/<slug>/
//!   problem.json      metadata: difficulty, limits, metric, seeds
//!   statement.md      the problem statement shown to solvers
//!   generator.c       seeded test generator (public + hidden seeds)
//!   checker.c         output checker (special judge), or null for exact match
//!   reference.cpp     the reference solution
//!   editorial.md      the editorial draft
//!   hints.md          progressive hints
//!   solutions/        naive.cpp, good.cpp, great.cpp
//! ```

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ProblemError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("problem metadata is invalid: {0}")]
    Metadata(String),
    #[error("missing required file: {0}")]
    MissingFile(String),
    #[error("validate failed at step {step}: {detail}")]
    Validation { step: String, detail: String },
}

/// Difficulty ladder, uniform across every field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    WarmUp,
    Easy,
    Medium,
    CacheAware,
    BeatTheReference,
    SimdLockFree,
    Budget,
    HardwareSpecific,
    ResearchGrade,
    HallOfFame,
}

impl Difficulty {
    /// The 1..10 level shown to users.
    pub fn level(self) -> u8 {
        match self {
            Difficulty::WarmUp => 1,
            Difficulty::Easy => 2,
            Difficulty::Medium => 3,
            Difficulty::CacheAware => 4,
            Difficulty::BeatTheReference => 5,
            Difficulty::SimdLockFree => 6,
            Difficulty::Budget => 7,
            Difficulty::HardwareSpecific => 8,
            Difficulty::ResearchGrade => 9,
            Difficulty::HallOfFame => 10,
        }
    }
}

/// What the problem is ranked on. Deterministic metrics only, for Phase 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankedMetric {
    Instructions,
    CacheMisses,
    BranchMisses,
    PeakMemory,
    Allocations,
    BinarySize,
}

/// Hard limits for a submission on this problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub time_ms: u64,
    pub memory_bytes: u64,
    pub output_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            time_ms: 10_000,
            memory_bytes: 512 * 1024 * 1024,
            output_bytes: 1 << 20,
        }
    }
}

/// Seed sets. `public` seeds are shipped so people can practise; `hidden` seeds
/// are generated fresh per run so hard-coding answers does not work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Seeds {
    pub public: Vec<u64>,
    pub hidden: Vec<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProblemMeta {
    pub id: String,
    pub title: String,
    pub category: String,
    pub difficulty: Difficulty,
    pub ranked_metric: RankedMetric,
    pub summary: String,
    #[serde(default)]
    pub limits: Limits,
    pub seeds: Seeds,
    /// Language of the reference solution.
    pub reference_language: String,
    /// `exact` compares bytes; `checker` runs checker.c.
    pub judge: Judge,
    /// Scale parameter passed to the generator, per size name.
    pub sizes: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Judge {
    /// Byte-for-byte comparison against the reference output.
    Exact,
    /// Run `checker.c` with (input, expected, actual).
    Checker,
}

/// A loaded problem folder.
#[derive(Debug, Clone)]
pub struct Problem {
    pub root: PathBuf,
    pub meta: ProblemMeta,
}

impl Problem {
    pub fn load(root: &Path) -> Result<Problem, ProblemError> {
        let root = root.canonicalize().map_err(|source| ProblemError::Io {
            path: root.to_path_buf(),
            source,
        })?;
        let meta_path = root.join("problem.json");
        let text = std::fs::read_to_string(&meta_path).map_err(|source| ProblemError::Io {
            path: meta_path.clone(),
            source,
        })?;
        let meta: ProblemMeta = serde_json::from_str(&text)
            .map_err(|e| ProblemError::Metadata(format!("problem.json: {e}")))?;
        let problem = Problem { root, meta };
        problem.check_required_files()?;
        Ok(problem)
    }

    fn check_required_files(&self) -> Result<(), ProblemError> {
        for rel in self.required_files() {
            let p = self.root.join(&rel);
            if !p.is_file() {
                return Err(ProblemError::MissingFile(rel));
            }
        }
        Ok(())
    }

    /// The files every problem must have, in the order `validate` checks them.
    pub fn required_files(&self) -> Vec<String> {
        let mut files = vec![
            "problem.json".to_string(),
            "statement.md".to_string(),
            "generator.c".to_string(),
            "brute_force.cpp".to_string(),
            "reference.cpp".to_string(),
            "editorial.md".to_string(),
            "hints.md".to_string(),
        ];
        if self.meta.judge == Judge::Checker {
            files.push("checker.c".to_string());
        }
        for name in self.meta.sizes.keys() {
            files.push(format!("solutions/{name}.cpp"));
        }
        files
    }

    /// Cross-check the metadata for internal consistency, before any code runs.
    pub fn check_metadata(&self) -> Result<(), ProblemError> {
        let m = &self.meta;
        if m.id.is_empty() {
            return Err(ProblemError::Metadata("id is empty".into()));
        }
        if m.seeds.public.is_empty() {
            return Err(ProblemError::Metadata(
                "seeds.public is empty: a problem with no public seeds cannot be practised".into(),
            ));
        }
        if m.seeds.hidden.is_empty() {
            return Err(ProblemError::Metadata(
                "seeds.hidden is empty: a problem with no hidden seeds can be hard-coded".into(),
            ));
        }
        // A hidden seed that is also public is not hidden.
        for s in &m.seeds.hidden {
            if m.seeds.public.contains(s) {
                return Err(ProblemError::Metadata(format!(
                    "seed {s} is in both seeds.public and seeds.hidden"
                )));
            }
        }
        if m.sizes.is_empty() {
            return Err(ProblemError::Metadata(
                "sizes is empty: at least one named size is needed".into(),
            ));
        }
        if m.sizes.len() < 3 {
            return Err(ProblemError::Metadata(
                "sizes needs at least naive/good/great to prove the metrics separate them".into(),
            ));
        }
        for (name, scale) in &m.sizes {
            if *scale == 0 {
                return Err(ProblemError::Metadata(format!("size {name:?} is zero")));
            }
        }
        if m.limits.time_ms == 0 || m.limits.memory_bytes == 0 {
            return Err(ProblemError::Metadata("limits must be non-zero".into()));
        }
        Ok(())
    }

    pub fn statement_path(&self) -> PathBuf {
        self.root.join("statement.md")
    }

    pub fn solution_path(&self, size: &str) -> PathBuf {
        self.root.join(format!("solutions/{size}.cpp"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> ProblemMeta {
        ProblemMeta {
            id: "top-k/heap-vs-sort".into(),
            title: "Top-K".into(),
            category: "data-structures".into(),
            difficulty: Difficulty::CacheAware,
            ranked_metric: RankedMetric::Instructions,
            summary: "Select the K largest of N".into(),
            limits: Limits::default(),
            seeds: Seeds {
                public: vec![1, 2, 3],
                hidden: vec![101, 102],
            },
            reference_language: "cpp".into(),
            judge: Judge::Exact,
            sizes: BTreeMap::from([
                ("naive".to_string(), 1),
                ("good".to_string(), 2),
                ("great".to_string(), 3),
            ]),
        }
    }

    fn problem_with(meta: ProblemMeta) -> Problem {
        Problem {
            root: PathBuf::from("."),
            meta,
        }
    }

    #[test]
    fn difficulty_maps_to_levels_one_through_ten() {
        let all = [
            Difficulty::WarmUp,
            Difficulty::Easy,
            Difficulty::Medium,
            Difficulty::CacheAware,
            Difficulty::BeatTheReference,
            Difficulty::SimdLockFree,
            Difficulty::Budget,
            Difficulty::HardwareSpecific,
            Difficulty::ResearchGrade,
            Difficulty::HallOfFame,
        ];
        for (i, d) in all.iter().enumerate() {
            assert_eq!(d.level(), (i + 1) as u8, "{d:?}");
        }
    }

    #[test]
    fn a_sound_problem_passes_metadata_check() {
        problem_with(meta()).check_metadata().unwrap();
    }

    #[test]
    fn overlapping_public_and_hidden_seeds_are_rejected() {
        let mut m = meta();
        m.seeds.hidden.push(1);
        let err = problem_with(m).check_metadata().unwrap_err();
        assert!(err.to_string().contains("both"));
    }

    #[test]
    fn empty_hidden_seeds_are_rejected() {
        let mut m = meta();
        m.seeds.hidden.clear();
        let err = problem_with(m).check_metadata().unwrap_err();
        assert!(err.to_string().contains("hard-coded"));
    }

    #[test]
    fn fewer_than_three_sizes_is_rejected() {
        let mut m = meta();
        m.sizes.remove("great");
        let err = problem_with(m).check_metadata().unwrap_err();
        assert!(err.to_string().contains("separate"));
    }

    #[test]
    fn checker_judge_requires_a_checker_file() {
        let mut m = meta();
        m.judge = Judge::Checker;
        let p = problem_with(m);
        assert!(p.required_files().contains(&"checker.c".to_string()));
    }

    #[test]
    fn exact_judge_does_not_require_a_checker_file() {
        let p = problem_with(meta());
        assert!(!p.required_files().contains(&"checker.c".to_string()));
    }
}
