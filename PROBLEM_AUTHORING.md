# Problem authoring

Create a problem under `problems/<category>/<slug>/`. The current executable
format uses a deterministic `generator.c` that accepts `seed scale`, a C++
`reference.cpp`, `statement.md`, `editorial.md`, `hints.md`, and three variants
under `solutions/` named for the size keys in `problem.json`.

`problem.json` declares the identifier, category, difficulty, ranking metric,
resource limits, public and hidden seed lists, exact-output or checker judging,
and named scales. Public and hidden seeds must be disjoint. Keep hidden seeds
out of public examples.

Before proposing a problem, verify that equal generator seeds produce identical
bytes, the reference handles every declared size, outputs have a stable format,
and the three variants represent distinct algorithmic tradeoffs. Run
`ccx validate problems/<category>/<slug>` after `ccx doctor` succeeds.

Every problem must provide `brute_force.cpp`. `ccx validate` checks generator
determinism for every declared size and public and hidden seed, cross-checks the
brute force against the reference on the smallest size across all seeds, and
checks each named solution at its corresponding size across all seeds. It
measures reference instruction counts for every size and seed, verifies that
size medians do not fall by more than 10%, and checks that three repeated
measurements of each variant have stable Cachegrind counters and allocation
records. It also
compares `naive` and `good` on one shared input. A `checker.c` file is required
when `judge` is `checker`; it is called with input, expected output, and
contestant output paths, and must exit zero for an accepted answer.
