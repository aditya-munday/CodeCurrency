# Editorial

Erasing the first element of a vector moves every remaining element one place.
Repeated front erasure can therefore take `O(Q^2)` time and uses contiguous
storage for the live queue.

`std::deque` supports efficient insertion and removal at both ends. Its
segmented storage avoids shifting the queue, though it uses multiple blocks.

A ring buffer stores the queue in one fixed-size array and tracks its head,
tail, and current length. Each operation takes `O(1)` time and the array is
allocated once. This implementation uses a branch at the end of the array to
wrap the indices without a remainder operation.
