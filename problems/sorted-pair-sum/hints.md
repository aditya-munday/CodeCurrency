# Hints

Progressive. Read one, try again, come back for the next.

## Hint 1 — the shape of the answer

The double loop is correct. Start there: get a working submission, then look at
the counters. A correct slow answer is a better starting point than a fast
guess, because `ccx diff` needs something to compare against.

## Hint 2 — what the counters say

Run `ccx measure --repeat 5` on the naive version. Look at `instructions` and
`cache_misses`. The instruction count grows with `n^2`; the cache miss rate is
high because the inner loop walks the array with a stride that defeats the
prefetcher.

## Hint 3 — the classic move

Sort the array. Then, with one index at each end, the count for the smallest
remaining element is known in one step: if `a[lo] + a[hi] <= T`, then every
element from `lo+1` to `hi` pairs with `lo`, and you can add `hi - lo` and
advance `lo`. Otherwise `hi` is too large and you retreat.

That is `O(n log n)` and it removes the inner loop entirely.

## Hint 4 — when sorting is not the best you can do

The values are bounded by `2n`. A comparison sort costs `n log n` comparisons;
a counting pass costs `n` increments. Then a prefix sum turns "how many values
are at most `X`" into a single array lookup, and the whole count becomes one
linear scan with no inner loop and no unpredictable branches.

Look at what that does to `branch_misses`.
