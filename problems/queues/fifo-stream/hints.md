# Hints

1. Removing element zero from a vector changes the position of every later
   element.
2. A deque exposes both ends directly and preserves FIFO order.
3. A ring buffer wraps its tail to zero after the final array slot. Track its
   length so full and empty states remain distinct.
