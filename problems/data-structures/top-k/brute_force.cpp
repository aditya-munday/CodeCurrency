#include <algorithm>
#include <cstdio>
#include <vector>

int main() {
    int n = 0;
    int k = 0;
    if (std::scanf("%d %d", &n, &k) != 2 || n < 2 || k < 1 || k >= n) {
        return 1;
    }
    std::vector<long long> values(static_cast<size_t>(n));
    for (long long &value : values) {
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
    }
    std::sort(values.begin(), values.end(), std::greater<long long>());
    for (int i = 0; i < k; i++) {
        std::printf("%lld\n", values[static_cast<size_t>(i)]);
    }
    return 0;
}
