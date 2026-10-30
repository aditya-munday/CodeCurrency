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
    std::vector<unsigned char> selected(static_cast<size_t>(n), 0);
    for (int rank = 0; rank < k; rank++) {
        int best = -1;
        for (int i = 0; i < n; i++) {
            if (!selected[static_cast<size_t>(i)] &&
                (best < 0 || values[static_cast<size_t>(i)] > values[static_cast<size_t>(best)])) {
                best = i;
            }
        }
        if (best < 0) {
            return 1;
        }
        selected[static_cast<size_t>(best)] = 1;
        std::printf("%lld\n", values[static_cast<size_t>(best)]);
    }
    return 0;
}
