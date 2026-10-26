# Count pairs summing to at most T

You are given `n` integers and a threshold `T`. Count the number of pairs
`(i, j)` with `i < j` such that `a[i] + a[j] <= T`.

## Input

```
n T
a[0] a[1] ... a[n-1]
```

`n` and `T` are on the first line. The `n` values follow on one line, separated
by single spaces.

## Output

One line: the number of qualifying pairs.

## Constraints

- `1 <= n <= 200000`
- `0 <= a[i] <= 2n`
- `0 <= T <= 2n`

The answer can exceed 32 bits: with `n = 200000`, up to about 2e10 pairs
qualify, so use a 64-bit counter.

## What makes this interesting

The double loop is the obvious answer and it is correct. On the largest size it
runs roughly 2e10 comparisons, most of them cache misses, and the instruction
count runs into the hundreds of billions.

Sorting first makes the count a linear scan. The counter makes the difference
unmistakable, which is the point: this problem is here to show you that the
metric notices what your eyes do not.
