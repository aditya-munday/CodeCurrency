#include <cstdio>
#include <vector>

int main() {
    int n = 0;
    if (std::scanf("%d", &n) != 1 || n < 1) {
        return 1;
    }
    std::vector<long long> values;
    values.reserve(static_cast<size_t>(n));
    for (int i = 0; i < n; i++) {
        long long value = 0;
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
        values.push_back(value);
    }
    long long checksum = 0;
    for (size_t i = 0; i < values.size(); i++) {
        checksum += static_cast<long long>(i + 1) * values[i];
    }
    std::printf("%lld\n", checksum);
    return 0;
}
