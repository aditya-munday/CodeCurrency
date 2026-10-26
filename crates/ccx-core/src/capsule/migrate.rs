//! Versioned capsule migrations.
//!
//! The schema only changes by adding a version and a migration from the
//! previous one. Old capsules stay readable forever; they are never silently
//! rewritten on disk.
//!
//! There is only one schema version today, so `migrate_value` has no steps yet.
//! The shape of a real migration is documented here so the first one is a
//! fill-in-the-blank rather than a redesign:
//!
//! ```ignore
//! if from == 1 {
//!     // v1 -> v2: `metrics` gained a mandatory `method` field.
//!     for m in value["metrics"].as_array_mut().unwrap() {
//!         m["method"] = json!("manual");
//!     }
//!     value["schema_version"] = json!(2);
//!     from = 2;
//! }
//! ```

use super::{CapsuleError, CURRENT_SCHEMA_VERSION};
use serde_json::Value;

/// Migrate a raw capsule document up to the current schema version.
pub fn migrate_value(value: Value, from: u32) -> Result<Value, CapsuleError> {
    if from > CURRENT_SCHEMA_VERSION {
        return Err(CapsuleError::UnsupportedVersion {
            found: from,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    // No migrations exist yet: v1 is the only version, so anything below the
    // current version has no registered path upward.
    let _ = value;
    Err(CapsuleError::Invalid(format!(
        "no migration path from schema_version {from} to {CURRENT_SCHEMA_VERSION}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn future_version_is_rejected() {
        let err = migrate_value(Value::Null, 99).unwrap_err();
        assert!(matches!(err, CapsuleError::UnsupportedVersion { .. }));
    }

    #[test]
    fn current_version_needs_no_migration() {
        // The loader handles the current version directly, so this path is only
        // reached for genuinely older documents.
        let err = migrate_value(Value::Null, 0).unwrap_err();
        assert!(matches!(err, CapsuleError::Invalid(_)));
    }
}
