# Editorial

## The three approaches, and what each one costs

**Naive (`O(n^2)`).** Two nested loops, comparing every pair. Correct, and the
instruction count grows quadratically. On `n = 200000` the inner loop executes
about 2e10 times. This is the baseline: not a strawman, but the answer most
people write first, and the one the platform is designed to make visibly
expensive.

**Good (`O(n log n)`).** Sort, then walk two pointers inward. When
`a[lo] + a[hi] <= T`, every element between `lo` and `hi` pairs with `lo`, so
`hi - lo` pairs are counted at once and `lo` advances. Otherwise `hi` retreats.
Each element moves at most once, so the count after sorting is linear.

The dominant cost is the sort. The pair count itself is a tight loop with
predictable branches, which is why the branch-miss count drops sharply even
though the instruction count is still in the millions.

**Great (`O(n + V)`).** Values are bounded by `2n`, so a counting pass replaces
the comparison sort. Build a frequency table, then a prefix sum. For each
distinct value `v` with frequency `f`:

- pairs where both elements are `v` contribute `choose(f, 2)` when `2v <= T`;
- pairs with a strictly smaller partner contribute `f * prefix[min(T - v, v - 1)]`.

This is one linear scan over the value range. There is no inner loop and no
data-dependent branch in the counting phase, so both the instruction count and
the branch-miss count fall below the two-pointer version.

## Why this problem exists

This is the first problem in the arena because the three approaches differ in
*wildly different* counters:

| approach | instructions | cache misses | branch misses |
| --- | --- | --- | --- |
| naive | grows as `n^2` | high, strided | moderate |
| good | grows as `n log n` | low after the sort | moderate |
| great | grows as `n` | low, sequential | very low |

A metric that cannot separate these three is not worth trusting. That is what
`ccx validate` checks: it measures the reference at every size and confirms the
cost grows with the size, so the problem can tell a better algorithm from a
worse one.

## Determinism

The generator uses xorshift64* written out in the source, not `rand()` from
libc. This matters: `rand()` differs between libc versions, so a generator that
used it would produce different input on different machines and no capsule
would ever replay. Every random choice in this problem comes from an algorithm
pinned in the repository.
