# Editorial

The direct approach scans the full input to find the next largest unused value,
then repeats that scan `K` times. It takes `O(NK)` time and `O(N)` storage. This
is useful as a small-input baseline.

A min-heap of size `K` keeps the smallest selected value at its root. The first
`K` values fill it; each later value either gets discarded or replaces the
root. This takes `O(N log K)` time and `O(K)` storage. Sorting the final heap
adds `O(K log K)` work.

Partition selection places the largest `K` values in a prefix using
`std::nth_element`, then sorts only that prefix. Its average work is
`O(N + K log K)` and it stores the input in `O(N)` space. The implementation
uses descending order for both partition and final sort.

The methods have different memory and instruction profiles. At the generated
ratio `K = N / 2`, partition selection may not win every metric on every host;
measurements should be read as results for the recorded compiler and input.
