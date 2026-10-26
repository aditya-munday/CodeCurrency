// Good solution: sort, then two pointers. O(n log n).
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
    long long count = 0;
    long long lo = 0, hi = n - 1;
    while (lo < hi) {
        if (a[static_cast<size_t>(lo)] + a[static_cast<size_t>(hi)] <= threshold) {
            count += hi - lo;
            lo++;
        } else {
            hi--;
        }
    }
    printf("%lld\n", count);
    return 0;
}
