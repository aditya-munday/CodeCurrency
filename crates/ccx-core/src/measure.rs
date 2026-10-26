//! Deterministic measurement: the trust layer.
//!
//! Layer 1 of Phase 1. The ranking signal is the set of metrics that do not
//! depend on which machine ran them: instruction counts, simulated cache and
//! branch misses, allocations, peak memory, and binary size. Wall-clock time is
//! recorded but labelled `indicative` and is never used for ranking.
//!
//! Primary method is `valgrind --tool=cachegrind`, which models the cache
//! deterministically, so two different machines produce the same numbers. When
//! a real hardware counter is available (`perf stat`), it is recorded
//! separately and labelled `measured`; it never replaces the simulated number.

use crate::capsule::{Metric, MetricKind, MetricMethod};
use serde::{Deserialize, Serialize};

/// Raw output of one cachegrind run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CachegrindCounts {
    pub instruction_refs: u64,
    pub data_refs: u64,
    pub data_reads: u64,
    pub data_writes: u64,
    pub instruction_cache_misses: u64,
    pub data_cache_misses: u64,
    pub branch_instructions: u64,
    pub branch_misses: u64,
}

impl CachegrindCounts {
    /// Total cache misses (instruction + data) as a single number.
    pub fn total_cache_misses(&self) -> u64 {
        self.instruction_cache_misses + self.data_cache_misses
    }

    /// Instructions retired, as modelled. This is the headline deterministic
    /// metric.
    pub fn instructions(&self) -> u64 {
        self.instruction_refs
    }

    pub fn to_metrics(&self) -> Vec<Metric> {
        vec![
            Metric::count(
                "instructions",
                self.instructions(),
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "cache_misses",
                self.total_cache_misses(),
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
            Metric::count(
                "branch_misses",
                self.branch_misses,
                MetricKind::Simulated,
                MetricMethod::Cachegrind,
            ),
        ]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("no cachegrind summary found in output")]
    MissingSummary,
    #[error("malformed cachegrind line: {0}")]
    Malformed(String),
    #[error("cachegrind reported an error: {0}")]
    Cachegrind(String),
}

/// Parse the summary block that `cachegrind` prints to stderr.
///
/// The format is a series of `name: value (rate%)` lines. We match on the
/// leading name so extra trailing detail (percentages, per-instruction rates)
/// does not break parsing.
pub fn parse_cachegrind_summary(stderr: &str) -> Result<CachegrindCounts, ParseError> {
    if stderr.contains("==error calling PR_SET_PTRACER") || stderr.contains("valgrind: failed") {
        return Err(ParseError::Cachegrind(stderr.lines().next().unwrap_or("").to_string()));
    }

    let mut counts = CachegrindCounts::default();
    let mut seen_any = false;

    for line in stderr.lines() {
        let line = line.trim();
        // Strip the `==pid== ` prefix valgrind adds.
        let line = match line.find("== ") {
            Some(idx) if line.starts_with("==") => line[idx + 3..].trim(),
            _ => line,
        };

        let (name, rest) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        // Cachegrind pads names with alignment spaces (`I   refs`), so collapse
        // runs of whitespace before matching.
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let value: u64 = match rest.trim().split_whitespace().next() {
            Some(tok) => match tok.replace(',', "").parse() {
                Ok(v) => v,
                Err(_) => continue,
            },
            None => continue,
        };

        match name.as_str() {
            "I refs" => {
                counts.instruction_refs = value;
                seen_any = true;
            }
            "D refs" => counts.data_refs = value,
            "D1 misses" => counts.data_cache_misses = value,
            "I1 misses" => counts.instruction_cache_misses = value,
            "Dr" => counts.data_reads = value,
            "Dw" => counts.data_writes = value,
            "Branches" => counts.branch_instructions = value,
            "Mispredicts" => counts.branch_misses = value,
            _ => continue,
        }
    }

    if !seen_any {
        return Err(ParseError::MissingSummary);
    }
    Ok(counts)
}

/// Parse `perf stat` output for the hardware counters we care about.
///
/// Returns `None` when no counter could be read, which is the honest answer on
/// a machine where `perf_event_paranoid` forbids access.
pub fn parse_perf_stat(stderr: &str) -> Option<(u64, u64, u64)> {
    let mut instructions = None;
    let mut cache_misses = None;
    let mut branch_misses = None;

    for line in stderr.lines() {
        let line = line.trim();
        let (value, name) = match line.split_once(' ') {
            Some(pair) => pair,
            None => continue,
        };
        let value: u64 = match value.replace(',', "").parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let name = name.trim();
        if name.starts_with("instructions") {
            instructions = Some(value);
        } else if name.starts_with("cache-misses") || name.starts_with("cache-misses") {
            cache_misses = Some(value);
        } else if name.starts_with("branch-misses") {
            branch_misses = Some(value);
        }
    }

    match (instructions, cache_misses, branch_misses) {
        (Some(i), Some(c), Some(b)) => Some((i, c, b)),
        (Some(i), _, _) => Some((i, cache_misses.unwrap_or(0), branch_misses.unwrap_or(0))),
        _ => None,
    }
}

/// Whether the host permits reading hardware performance counters.
pub fn perf_available(perf_event_paranoid: Option<i32>) -> bool {
    matches!(perf_event_paranoid, Some(v) if v <= 2)
}

/// Read `/proc/sys/kernel/perf_event_paranoid` on the host.
pub fn read_perf_event_paranoid() -> Option<i32> {
    std::fs::read_to_string("/proc/sys/kernel/perf_event_paranoid")
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
==12345== I   refs:      1,234,567
==12345== I1  misses:        1,024
==12345== D   refs:        456,789  (300,000 rd + 156,789 wr)
==12345== D1  misses:        2,048
==12345== Branches:         50,000  (40,000 cond + 10,000 ind)
==12345== Mispredicts:       1,500
"#;

    #[test]
    fn parses_summary_block() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        assert_eq!(c.instruction_refs, 1_234_567);
        assert_eq!(c.instruction_cache_misses, 1_024);
        assert_eq!(c.data_refs, 456_789);
        assert_eq!(c.data_cache_misses, 2_048);
        assert_eq!(c.branch_instructions, 50_000);
        assert_eq!(c.branch_misses, 1_500);
    }

    #[test]
    fn totals_are_sums() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        assert_eq!(c.total_cache_misses(), 1_024 + 2_048);
        assert_eq!(c.instructions(), 1_234_567);
    }

    #[test]
    fn metrics_are_labelled_simulated() {
        let c = parse_cachegrind_summary(SAMPLE).unwrap();
        for m in c.to_metrics() {
            assert_eq!(m.kind, MetricKind::Simulated);
            assert_eq!(m.method, MetricMethod::Cachegrind);
        }
    }

    #[test]
    fn missing_summary_is_an_error_not_a_zero() {
        let err = parse_cachegrind_summary("nothing useful here").unwrap_err();
        assert!(matches!(err, ParseError::MissingSummary));
    }

    #[test]
    fn parses_perf_stat() {
        let text = "      1,234,567      instructions\n         12,345      cache-misses\n          1,234      branch-misses\n";
        let (i, c, b) = parse_perf_stat(text).unwrap();
        assert_eq!(i, 1_234_567);
        assert_eq!(c, 12_345);
        assert_eq!(b, 1_234);
    }

    #[test]
    fn perf_availability_follows_paranoid_level() {
        assert!(perf_available(Some(0)));
        assert!(perf_available(Some(2)));
        assert!(!perf_available(Some(3)));
        assert!(!perf_available(None));
    }
}
