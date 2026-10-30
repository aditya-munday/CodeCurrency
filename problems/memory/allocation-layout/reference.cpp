#include <cstdio>
#include <vector>

int main() {
    int n = 0;
    if (std::scanf("%d", &n) != 1 || n < 1) {
        return 1;
    }
    std::vector<long long> values(static_cast<size_t>(n));
    for (long long &value : values) {
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
    }
    long long checksum = 0;
    for (int i = 0; i < n; i++) {
        checksum += static_cast<long long>(i + 1) * values[static_cast<size_t>(i)];
    }
    std::printf("%lld\n", checksum);
    return 0;
}
