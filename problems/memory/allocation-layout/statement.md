# Build and Traverse a Record Chain

The first line contains `N`, followed by `N` signed integer values. Build a
sequence in input order and print the weighted checksum
`sum((i + 1) * value[i])`.

The input satisfies `1 <= N <= 200000`, and every value is between `-1000` and
`1000`. The checksum fits in a signed 64-bit integer.

The task allows different storage strategies that preserve input order. Compare
allocating each record separately, storing linked records in one reserved
pool, and storing values contiguously. Allocation count and allocated bytes
provide additional measured context for the instruction count.
