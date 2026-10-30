#include <algorithm>
#include <cstdio>
#include <functional>
#include <queue>
#include <vector>

int main() {
    int n = 0;
    int k = 0;
    if (std::scanf("%d %d", &n, &k) != 2 || n < 2 || k < 1 || k >= n) {
        return 1;
    }
    std::priority_queue<long long, std::vector<long long>, std::greater<long long>> top;
    for (int i = 0; i < n; i++) {
        long long value = 0;
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
        if (top.size() < static_cast<size_t>(k)) {
            top.push(value);
        } else if (value > top.top()) {
            top.pop();
            top.push(value);
        }
    }
    std::vector<long long> result;
    result.reserve(static_cast<size_t>(k));
    while (!top.empty()) {
        result.push_back(top.top());
        top.pop();
    }
    std::sort(result.begin(), result.end(), std::greater<long long>());
    for (long long value : result) {
        std::printf("%lld\n", value);
    }
    return 0;
}
