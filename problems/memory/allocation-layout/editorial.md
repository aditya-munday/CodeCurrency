# Editorial

Allocating one node at a time performs `N` heap allocations. Each node also
stores a pointer, and traversal follows those pointers through separately
allocated memory. The linked representation makes locality and allocator work
visible in the counters.

A reserved vector of nodes uses one backing allocation while keeping the same
index-linked traversal. This removes per-node allocator calls while retaining
the link metadata.

A contiguous vector of values stores no link metadata and needs only a
sequential pass for the checksum. It uses one backing allocation and has the
smallest representation of these three approaches.

Valgrind Memcheck xtree reports heap allocation blocks and bytes in a separate
pass, so that instrumentation does not change the Cachegrind or native timing
results. It shows the allocation-count difference between the node-by-node and
pooled solutions; instruction and cache behavior explain the cost of traversal
and representation.
