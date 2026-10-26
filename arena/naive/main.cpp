// Naive solution: the double loop. O(n^2).
//
// Correct, but the inner loop is a cache-hostile strided scan and the
// instruction count explodes with n. This is the baseline the platform is
// meant to embarrass.
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
    long long count = 0;
    for (long long i = 0; i < n; i++) {
        for (long long j = i + 1; j < n; j++) {
            if (a[static_cast<size_t>(i)] + a[static_cast<size_t>(j)] <= threshold) {
                count++;
            }
        }
    }
    printf("%lld\n", count);
    return 0;
}
