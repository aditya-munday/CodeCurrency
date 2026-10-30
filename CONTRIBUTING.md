# Contributing

CodeCurrency is focused on reproducible systems-performance experiments. Keep
changes small enough to review and include the evidence needed to reproduce
measurement or safety claims.

## Development

Install Rust 1.85 or later. Run `cargo fmt --all -- --check` and
`cargo test --release` before opening a pull request. Engine changes that touch
container behavior should also run `scripts/safety-suite.sh` where Docker is
available.

## Measurement changes

Never label a modeled value as measured. Add or update capsule migrations when
changing the serialized schema. Record toolchain image digests, fixed inputs,
and the exact measurement method. Do not claim cross-machine stability from
samples taken on one host.

## Problem changes

Use the structure and constraints in [PROBLEM_AUTHORING.md](PROBLEM_AUTHORING.md).
Keep generators deterministic for the same seed and make statements,
references, limits, and sample solutions agree. Include evidence that the
problem distinguishes the intended performance tradeoff.

## Pull requests

Use a feature branch and open a pull request. Include a concise summary,
commands run, and known limitations. Do not publish toolchain images or claim a
Phase 1 gate complete without the corresponding reproducible evidence.
