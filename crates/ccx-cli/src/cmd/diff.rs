//! `ccx diff` — compare two capsules.
//!
//! A diff answers "did this change actually help, and on which counter". It
//! refuses to call a change an improvement when it is smaller than the recorded
//! spread, because a difference inside the noise is not a difference.

use crate::cmd::{self, Fixable};
use crate::Format;
use ccx_core::capsule::Capsule;
use ccx_core::explain::{self, CapsuleDiff, Significance};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DiffReport {
    pub diff: CapsuleDiff,
    /// Metrics where the change is larger than the recorded spread.
    pub real_changes: Vec<String>,
    /// Metrics where the change is inside the noise.
    pub noise_only: Vec<String>,
    pub inconclusive: Vec<String>,
    /// Whether the two capsules are even comparable.
    pub comparable: bool,
    pub warnings: Vec<String>,
}

fn load(path: &std::path::Path) -> anyhow::Result<Capsule> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        Fixable::new(format!("cannot read {}: {e}", path.display()))
            .fix("pass two capsule.json paths")
            .into_error()
    })?;
    Capsule::from_json(&text).map_err(|e| {
        Fixable::new(format!("{} is not a valid capsule: {e}", path.display()))
            .fix("regenerate it with `ccx measure`")
            .into_error()
    })
}

pub fn run(format: Format, a: &std::path::Path, b: &std::path::Path) -> anyhow::Result<()> {
    let before = load(a)?;
    let after = load(b)?;
    let mut diff = explain::diff(&before, &after);

    // A diff across problems, toolchains, or machines is not a comparison of
    // the code, so say so rather than implying the change caused the delta.
    let mut warnings = Vec::new();
    if before.problem.id != after.problem.id {
        warnings.push(format!(
            "different problems ({} vs {}): the comparison is not meaningful",
            before.problem.id, after.problem.id
        ));
    }
    if before.source.language != after.source.language {
        warnings.push("different languages".to_string());
    }
    if before.host.fingerprint != after.host.fingerprint {
        warnings.push(
            "recorded on different machines: machine-dependent metrics are not comparable".into(),
        );
    }

    let comparable = before.problem.id == after.problem.id
        && before.toolchain.same_environment(&after.toolchain)
        && before.source.language == after.source.language;

    // Only deterministic metrics can be called a real change; an indicative
    // metric inside its interval is noise by construction.
    let mut real_changes = Vec::new();
    let mut noise_only = Vec::new();
    let mut inconclusive = Vec::new();
    for d in &diff.deltas {
        match d.significant {
            Significance::Real => real_changes.push(d.name.clone()),
            Significance::WithinNoise => noise_only.push(d.name.clone()),
            Significance::Unknown => inconclusive.push(d.name.clone()),
        }
    }

    // Stable ordering for display and tests.
    diff.deltas.sort_by(|x, y| x.name.cmp(&y.name));
    real_changes.sort();
    noise_only.sort();
    inconclusive.sort();

    let report = DiffReport {
        diff,
        real_changes,
        noise_only,
        inconclusive,
        comparable,
        warnings,
    };

    cmd::emit(
        format,
        || {
            println!(
                "{} -> {}",
                &report.diff.from_id.clone().unwrap_or_default()
                    [..12.min(report.diff.from_id.as_deref().unwrap_or("").len())],
                &report.diff.to_id.clone().unwrap_or_default()
                    [..12.min(report.diff.to_id.as_deref().unwrap_or("").len())],
            );
            for w in &report.warnings {
                println!("  warning: {w}");
            }
            println!();
            println!(
                "  {:<18} {:>14} {:>14} {:>12}  {}",
                "metric", "before", "after", "change", "verdict"
            );
            for d in &report.diff.deltas {
                let pct = d
                    .delta_pct
                    .map(|p| format!("{p:+.1}%"))
                    .unwrap_or_else(|| "n/a".into());
                let verdict = match d.significant {
                    Significance::Real => "real",
                    Significance::WithinNoise => "noise",
                    Significance::Unknown => "unknown",
                };
                println!(
                    "  {:<18} {:>14} {:>14} {:>12}  {verdict}",
                    d.name,
                    format!("{:.0}", d.before),
                    format!("{:.0}", d.after),
                    pct,
                );
            }
            if !report.real_changes.is_empty() {
                println!();
                println!("  real change in: {}", report.real_changes.join(", "));
            }
            if !report.noise_only.is_empty() {
                println!("  within noise:   {}", report.noise_only.join(", "));
            }
            if !report.inconclusive.is_empty() {
                println!("  inconclusive:   {}", report.inconclusive.join(", "));
            }
        },
        &report,
    )?;

    if !report.comparable {
        return Err(Fixable::new("the two capsules are not comparable")
            .fix("compare capsules of the same problem, language, and toolchain")
            .into_error());
    }
    Ok(())
}
