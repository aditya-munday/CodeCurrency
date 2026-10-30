#include "pair_count.hpp"

#include <cstdio>
#include <vector>

int main() {
    long long n = 0;
    long long threshold = 0;
    if (std::scanf("%lld %lld", &n, &threshold) != 2 || n <= 0) {
        return 1;
    }

    std::vector<long long> values(static_cast<std::size_t>(n));
    for (long long index = 0; index < n; ++index) {
        if (std::scanf("%lld", &values[static_cast<std::size_t>(index)]) != 1) {
            return 1;
        }
    }

    std::printf("%lld\n", count_pairs(values, threshold));
    return 0;
}
