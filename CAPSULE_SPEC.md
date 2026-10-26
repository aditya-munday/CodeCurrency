# CAPSULE_SPEC.md — Experiment Capsule, version 1

A **capsule** is the unit of evidence on CodeCurrency. It is a small, versioned
JSON document that fully describes one measured run of one solution to one
problem, such that anyone can re-run it and get the same answer.

Design rules:

1. **Self-describing.** Everything needed to reproduce is inside the capsule.
   No external lookup, no "you also need my local config".
2. **Content-addressed.** Source files and the problem statement are stored as
   SHA-256 hashes; the capsule references the content, it does not embed it.
3. **Pinned, never floating.** Toolchains are referenced by image digest. A
   version string is documentation; a digest is the contract.
4. **Labels are mandatory.** Every metric carries `kind`: `measured` (a real
   hardware or OS counter), `simulated` (a deterministic model, e.g. cachegrind),
   or `indicative` (wall-clock, machine-dependent).
5. **Validated on load.** A capsule that does not validate is rejected, not
   best-effort parsed. Unknown fields are a hard error at v1.
6. **Versioned migrations.** The schema only changes by adding a new version and
   a migration from the previous one. Old capsules never become unreadable.

---

## 1. Top level

```jsonc
{
  "schema_version": 1,          // integer, bumped only with a migration
  "capsule_id": "…",            // sha256 of the canonical body (see §9)
  "created_at": "2026-10-26T11:00:00Z",  // RFC 3339, UTC
  "problem":   { … },           // §2
  "source":    { … },           // §3
  "toolchain": { … },           // §4
  "build":     { … },           // §5
  "harness":   { … },           // §6
  "input":     { … },           // §7
  "host":      { … },           // §8
  "verdict":   "accepted",      // §10
  "metrics":   [ … ],           // §11
  "repeatability": { … }        // §12, optional
}
```

## 2. `problem`

| field | type | notes |
|---|---|---|
| `id` | string | slug, e.g. `top-k/heap-vs-sort` |
| `statement_sha256` | string | hex, of the canonical statement file |

## 3. `source`

| field | type | notes |
|---|---|---|
| `language` | enum | `c`, `cpp`, `asm-x86-64` (Phase 1 set) |
| `entry` | string | relative path of the translation unit to compile |
| `files` | map<string,string> | relative path → `sha256:<hex>` |
| `source_sha256` | string | hex, over the sorted `files` map (§9) |

## 4. `toolchain`

| field | type | notes |
|---|---|---|
| `name` | enum | `gcc`, `clang` |
| `version` | string | compiler-reported version, documentation only |
| `image` | string | `docker.io/library/ccx-gcc@sha256:…` |
| `image_digest` | string | `sha256:…`, the contract |
| `libc` | string | e.g. `glibc 2.41` |

## 5. `build`

| field | type | notes |
|---|---|---|
| `argv` | string[] | full compiler argv after the allow-list check |
| `opt_level` | string | e.g. `-O2` (extracted, for grouping) |
| `env` | map<string,string> | *fixed* environment (see §13) |
| `argv_program` | string[] | program arguments passed at run time |
| `stdin_sha256` | string\|null | hash of piped stdin, null if none |
| `stack_bytes` | integer | `RLIMIT_STACK`, fixed across runs |

## 6. `harness`

| field | type | notes |
|---|---|---|
| `version` | string | semver of the measurement harness |
| `metric_method` | enum | `cachegrind`, `perf`, `manual` |
| `ruleset` | string | hash of the rule set used for hints (§Layer 8) |

## 7. `input`

| field | type | notes |
|---|---|---|
| `generator` | string | id of the seeded generator |
| `seed` | integer | the seed actually used |
| `seed_set` | enum | `public` (practice) or `hidden` (ranking) |
| `size` | integer\|null | problem-specific scale parameter |

## 8. `host`

| field | type | notes |
|---|---|---|
| `cpu_model` | string | from `/proc/cpuinfo` |
| `arch` | string | `x86_64` |
| `os` | string | `/etc/os-release` `PRETTY_NAME` |
| `kernel` | string | `uname -r` |
| `nproc` | integer | usable CPUs at run time |
| `frequency_policy` | string | governor, or `unknown` |
| `fingerprint` | string | hex, hash of the above (§9) |

`host` is recorded but **must not affect** deterministic metrics. If it does,
that is a bug in the metric, not a property of the host.

## 9. Hashing

All hashes are SHA-256, lowercase hex, no prefix except in the `files` map
where the form `sha256:<hex>` is used.

- `source_sha256`: over the UTF-8 bytes of `"<path>\0<hex>\n"` for each entry,
  paths sorted lexicographically ascending.
- `host.fingerprint`: over `"<cpu_model>|<arch>|<os>|<kernel>|<nproc>"`.
- `capsule_id`: over the canonical JSON of the capsule body with `capsule_id`
  removed and all object keys sorted lexicographically, no insignificant
  whitespace.

`capsule_id` therefore changes if *any* input to the experiment changes, and
never changes if nothing does.

## 10. Verdict model

One of: `accepted`, `wrong_answer`, `time_limit`, `memory_limit`,
`runtime_error`, `build_error`, `rejected_unsafe`.

A capsule with any verdict other than `accepted` may omit `metrics` for the
run itself but must still record the attempt.

## 11. `metrics`

An array so that a single run can carry several metrics and so the shape stays
stable as metrics are added.

```jsonc
{
  "name": "instructions",
  "value": 1234567,
  "unit": "count",
  "kind": "simulated",        // measured | simulated | indicative
  "method": "cachegrind",     // cachegrind | perf | getrusage | stat | manual
  "uncertainty": null          // or { "kind": "stdev"|"ci95"|"range", "value": … }
}
```

Phase 1 metric set:

| name | unit | kind | method |
|---|---|---|---|
| `instructions` | count | simulated or measured | cachegrind / perf |
| `cache_misses` | count | simulated | cachegrind |
| `branch_misses` | count | simulated | cachegrind |
| `peak_memory` | bytes | measured | getrusage |
| `allocations` | count | measured | allocator shim |
| `allocated_bytes` | bytes | measured | allocator shim |
| `binary_size` | bytes | measured | stat |
| `wall_clock` | nanoseconds | indicative | repeated fresh-process timing |

Deterministic metrics are the ranking signal. `wall_clock` is explicitly
`indicative` and is **never** used for ranking in Phase 1.

## 12. `repeatability`

Present when a metric was measured more than once.

```jsonc
{
  "runs": 20,
  "per_metric": {
    "instructions": { "min": …, "max": …, "median": …, "spread_pct": 0.0 }
  }
}
```

`spread_pct = (max - min) / median * 100`. The achieved tolerance is documented
in `METHODOLOGY.md`, not asserted here.

## 13. Fixed environment

Compilation and execution both run with a **fixed** environment so counts do not
drift between runs. The canonical set (all other variables are dropped):

```
PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
LANG=C
LC_ALL=C
TZ=UTC
SOURCE_DATE_EPOCH=0
```

## 14. Migration policy

- Adding an optional field with a safe default: keep the version, note it in the
  changelog. Readers must tolerate unknown *optional* additions only where the
  field is declared `optional`; otherwise unknown fields are rejected.
- Any other change: increment `schema_version`, ship `migrate_v{n}_to_v{n+1}`,
  and keep old capsules loadable by migrating on read.
- A capsule is never silently rewritten on disk by a migration; migration output
  is written to a new file.
