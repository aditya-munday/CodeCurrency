# Performance finding: pair counting

This run compares the checked-in naive pair scan with the sort-and-two-pointer
solution on one identical generated input. Both capsules use GCC 14.2.0 at
`-O2`, the pinned `ccx-gcc` image, size `naive` (800 values), seed `1`, and the
same input hash. Each capsule records twenty fresh-process samples of the same
binary using harness 0.18.0's fixed Cachegrind model: 32 KiB 8-way I1/D1
caches with 64-byte lines. The reported cache-miss count is I1 plus D1 misses;
LL misses are excluded because Cachegrind uses the host-detected last-level
cache despite an explicit LL setting.
Each capsule also records three clean candidate builds, requiring identical
executable SHA-256 digests, with build timing distributions.

The [naive source](arena/naive/main.cpp) checks every pair in nested loops. The
[good source](arena/good/main.cpp) sorts the values, then advances one of two
pointers per comparison. `ccx asm --dir arena/naive` and
`ccx asm --dir arena/good` produce GCC assembly annotated with source lines;
each command prints the path to its complete assembly artifact alongside the
terminal preview. The repeated pair loop maps to `main.cpp:21–27`, while the replacement path maps
to the sort at `main.cpp:17` and the pointer loop at `main.cpp:20–26`.

## Results

| Metric | Naive | Sort and two pointers | Change | Label |
| --- | ---: | ---: | ---: | --- |
| Instructions | 5,317,517 | 2,506,969 | −52.85% | simulated, Cachegrind |
| Memory references | 6,520,988 | 3,409,423 | −47.72% | simulated, Cachegrind |
| Branches | 714,278 | 407,255 | −42.98% | simulated, Cachegrind |
| Branch misses | 15,727 | 18,868 | +19.97% | simulated, Cachegrind |
| Cache misses | 17,683 | 17,708 | +0.14% | simulated, fixed Cachegrind model |
| Allocations | 4 | 4 | 0% | measured, Valgrind Memcheck xtree |
| Allocated bytes | 88,320 | 88,320 | 0% | measured, Valgrind Memcheck xtree |
| Binary size | 16,560 B | 16,904 B | +2.08% | measured, filesystem stat |
| Execution wall time | 1.847 ms ± 0.268 ms | 1.904 ms ± 0.441 ms | within noise | indicative, bootstrap interval |
| C++ reference wall time | 1.541 ms ± 0.208 ms | 1.651 ms ± 0.194 ms | within noise | indicative, bootstrap interval |
| Paired reference/candidate ratio | 0.876 ± 0.209 | 0.915 ± 0.164 | within noise | indicative, per-pair ratio; above 1 favours candidate |
| Clean build wall time | 340.195 ms ± 9.051 ms | 414.340 ms ± 5.505 ms | inconclusive; +21.8% observed | measured, 3 clean builds |
| Build user CPU time | 290.378 ms ± 19.796 ms | 342.875 ms ± 12.163 ms | inconclusive; +18.1% observed | measured, 3 clean builds |
| Build system CPU time | 39.915 ms ± 14.883 ms | 68.442 ms ± 12.873 ms | inconclusive; +71.5% observed | measured, 3 clean builds |

The deterministic instruction count is 2.12 times lower for the sort-and-two-
pointers solution. The memory-reference count falls by 47.72%. This agrees with
the source and mapped assembly: the naive version visits every pair, while the
optimized version sorts once and processes the sorted sequence with a linear
pointer walk.

The branch counters show a tradeoff. Total branches fall by 42.98%, while
branch misses rise by 19.97%; the simulated miss rate moves from 2.20% to 4.63%.
That pattern is consistent with data-dependent sorting and pointer decisions,
and the counters do not attribute misses to a particular source line. Cache
misses increase by 25 in the fixed Cachegrind model. Allocation count and
allocated bytes stay equal, and the optimized binary is 344 bytes larger.

The candidate and reference wall-time intervals overlap. The paired ratio
medians are 0.876 ± 0.209 for naive and 0.915 ± 0.164 for good; `ccx diff`
classifies the ratio change as within noise. This sample does not establish a
statistically clear wall-time speedup. Each pair is a fresh candidate and C++
reference process on the same input, with order alternating between samples.
The ratio is indicative context and does not replace the deterministic
instruction result or establish a general ranking.

The three-build median wall time was 21.8% higher for the sort-and-two-pointers
version. `ccx diff` marks build-time comparisons inconclusive because these
three samples do not establish a general compile-time difference. The good
solution's observed build-time spread was 2.66%, compared with 5.32% for naive.
These short samples are host- and compiler-dependent and are not a
ranking signal. Both capsules hash the candidate executable, and
replay requires all repeated builds to reproduce those exact bytes before
measuring them.

The runner also recorded measured OS counters. Median user CPU time was 1.446
ms for naive and 1.540 ms for good; median system CPU time was zero for both.
Minor page faults were 165 and 167; voluntary context switches were 1 for both,
and involuntary context switches were 3 for both. Block-I/O operation
counts were zero in both samples.
These process counters do not identify file-level I/O latency or network
activity.

## Replay and diff

The schema-14, harness-0.18.0 capsules are available as [naive](arena/naive/capsule.phase1.v0.18.json)
and [optimized](arena/good/capsule.phase1.v0.18.json). Their IDs are
`fee349a5d91c59df14ed83f3fdd5705acb47ce0301090d4276f787c057e938af` and
`00f9fdca00419c3a8da1ba687136ddbd27be1ea858b9e6d9da34fa29ce7f54af`.

Replay both capsules and inspect the metric deltas with:

```sh
target/release/ccx replay arena/naive/capsule.phase1.v0.18.json
target/release/ccx replay arena/good/capsule.phase1.v0.18.json
target/release/ccx diff arena/naive/capsule.phase1.v0.18.json arena/good/capsule.phase1.v0.18.json
```

Both capsules were recorded with the current pinned environment and each
records twenty same-binary samples plus three byte-identical clean builds.
Replay verifies candidate and reference executable digests and the reference
compiler argv, then reproduces deterministic counters and allocation metrics.
Both replay reports had `deterministic_match: true`. The naive replay had
`machine_dependent_match: false` because its replay build wall time and user CPU
time fell outside their recorded sample intervals. Candidate and reference
wall-time samples stayed within their intervals. The good replay matched all
machine-dependent intervals. Build-time drift did not affect deterministic
reproduction. The host was a 12th Gen
Intel Core i5-12450HX running Ubuntu 26.04 under WSL2; the capsules store the
full host fingerprint and measurement metadata. The older `capsule.phase1.json`
files remain as harness-0.16.0 historical samples. Current replay requires the
recorded harness version, so use the 0.18.0 pair above for replay.

## Generated-test throughput sample

`ccx test --dir arena/good --cases 1000 --seed 40000 --size naive --jobs 4`
completed 1,000 generated cases plus the nine fixed public cases in 17.09
seconds on that host. The generated cases used fresh generator, reference, and
candidate processes under the per-process runner; the candidate process wall
time median was 1.019 ms. The end-to-end rate of 58.5 generated cases per
second includes the initial build and fixed public cases, so it is not a
steady-state rate. One sample does not establish a million-case completion
time or a throughput guarantee.

To collect new twenty-run samples for the same workload, use:

```sh
target/release/ccx measure --dir arena/naive --size naive --seed 1 --repeat 20 --out arena/naive/reproduced.json
target/release/ccx measure --dir arena/good --size naive --seed 1 --repeat 20 --out arena/good/reproduced.json
target/release/ccx replay arena/naive/reproduced.json
target/release/ccx replay arena/good/reproduced.json
target/release/ccx diff arena/naive/reproduced.json arena/good/reproduced.json
```

## A third approach: counting by value

The [great source](arena/great/main.cpp) uses the problem's value bound
`0 <= a[i] <= 2n` to count frequencies and build a prefix sum instead of
sorting. Its schema-14, harness-0.18.0 capsule is
[available here](evidence/sorted-pair-sum-great.json). It uses the exact same
public seed `1`, size `800`, stdin SHA-256, GCC 14.2.0 image, and host as the
naive and good capsules above. It records twenty samples, three byte-identical
builds, and its measurement report marked instructions eligible with 0.00%
spread. The capsule ID is
`ee42b45f21990ff8e2abefd943b6ef6c86a31d8732bd84512c678e586cfa102a`.

| Metric | Sort and two pointers | Count and prefix sum | Change |
| --- | ---: | ---: | ---: |
| Instructions | 2,506,969 | 2,480,374 | −1.06% |
| Memory references | 3,409,423 | 3,375,295 | −1.00% |
| Branches | 407,255 | 402,480 | −1.17% |
| Branch misses | 18,868 | 15,782 | −16.36% |
| Cache misses | 17,708 | 18,239 | +3.00% |
| Allocations | 4 | 6 | +50% |
| Allocated bytes | 88,320 | 113,904 | +28.97% |
| Binary size | 16,904 B | 16,672 B | −1.37% |

The deterministic counters show a small instruction reduction and fewer branch
misses, alongside more cache misses and allocation traffic. The source-mapped
assembly identifies the frequency fill and prefix passes at
[`main.cpp:27`](arena/great/main.cpp#L27) and
[`main.cpp:34`](arena/great/main.cpp#L34); the quadratic pair loop is absent.
On this small input, the extra frequency and prefix storage consumes much of
the asymptotic advantage. This is evidence for choosing an algorithm based on
the input bounds and scale, not evidence that counting is always faster.

The Great candidate's wall-clock median was `1.793 ms ± 0.263 ms`; the good
candidate measured `1.904 ms ± 0.441 ms`. The confidence intervals overlap,
and `ccx diff` classifies the paired-ratio change as `within_noise`, so this
does not establish a wall-clock winner. The instruction and branch counters
are the supported deterministic comparison for this result.

The saved capsule replayed on its recording host with both
`deterministic_match: true` and `machine_dependent_match: true`. Reproduce the
measurement, replay, and diff with:

```sh
target/release/ccx measure --dir arena/great --size naive --seed 1 --repeat 20 \
  --out evidence/sorted-pair-sum-great.json
target/release/ccx replay evidence/sorted-pair-sum-great.json --dir arena/great
target/release/ccx diff arena/good/capsule.phase1.v0.18.json \
  evidence/sorted-pair-sum-great.json
```
