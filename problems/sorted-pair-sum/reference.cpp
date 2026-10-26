// Reference solution: sort, then two pointers. O(n log n).
//
// This is the "great" answer. It is deliberately the best-known approach rather
// than the simplest, because the reference defines the target the counters are
// measured against.
#include <algorithm>
#include <cstdio>
#include <vector>

int main() {
    long long n = 0, threshold = 0;
    if (scanf("%lld %lld", &n, &threshold) != 2) {
        return 1;
    }
    std::vector<long long> a(static_cast<size_t>(n));
    for (long long i = 0; i < n; i++) {
        if (scanf("%lld", &a[static_cast<size_t>(i)]) != 1) {
            return 1;
        }
    }
    std::sort(a.begin(), a.end());

    // Count pairs with a[i] + a[j] <= threshold, i < j.
    long long count = 0;
    long long lo = 0, hi = n - 1;
    while (lo < hi) {
        if (a[static_cast<size_t>(lo)] + a[static_cast<size_t>(hi)] <= threshold) {
            // Every j in (lo, hi] pairs with lo.
            count += hi - lo;
            lo++;
        } else {
            hi--;
        }
    }
    printf("%lld\n", count);
    return 0;
}
