# Process a FIFO Event Stream

The first line contains the number of operations `Q`. Each following line is
one operation: `1 X` enqueues the signed integer `X`; `2` dequeues and prints
the oldest queued value. Print one value per dequeue operation.

The input guarantees that a dequeue never occurs on an empty queue. It contains
at most `200000` operations, and every value is between `-1000000` and
`1000000`.

The generated workload enqueues two values for each dequeue. Compare shifting
the remaining vector contents, the standard library deque, and a contiguous
ring buffer with storage allocated once.
