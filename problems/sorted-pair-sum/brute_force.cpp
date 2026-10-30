#include <cstdio>
#include <vector>

int main() {
    long long n = 0, threshold = 0;
    if (scanf("%lld %lld", &n, &threshold) != 2) {
        return 1;
    }
    std::vector<long long> values(static_cast<size_t>(n));
    for (long long i = 0; i < n; ++i) {
        if (scanf("%lld", &values[static_cast<size_t>(i)]) != 1) {
            return 1;
        }
    }
    long long count = 0;
    for (long long i = 0; i < n; ++i) {
        for (long long j = i + 1; j < n; ++j) {
            if (values[static_cast<size_t>(i)] + values[static_cast<size_t>(j)] <= threshold) {
                ++count;
            }
        }
    }
    printf("%lld\n", count);
    return 0;
}
