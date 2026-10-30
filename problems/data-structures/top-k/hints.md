# Hints

1. Finding one maximum is simple. What happens to the running time if you scan
   all `N` values again for each of the `K` results?
2. A min-heap of size `K` keeps the smallest selected value at its root. Use it
   to decide whether each new value belongs in the result.
3. `std::nth_element` can partition the input around the `K`th largest value;
   sort the selected prefix before printing it.
