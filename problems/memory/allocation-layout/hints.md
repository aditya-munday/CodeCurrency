# Hints

1. The checksum depends on the order of values, not on the choice of storage.
2. A node-per-record linked list is easy to build, but each node requests heap
   storage separately.
3. Reserve a node pool before linking indices. Then compare that layout with a
   plain contiguous array of values.
