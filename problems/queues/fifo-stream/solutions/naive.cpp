#include <cstdio>
#include <vector>

int main() {
    int operations = 0;
    if (std::scanf("%d", &operations) != 1 || operations < 1) {
        return 1;
    }
    std::vector<long long> queue;
    for (int i = 0; i < operations; i++) {
        int kind = 0;
        if (std::scanf("%d", &kind) != 1) {
            return 1;
        }
        if (kind == 1) {
            long long value = 0;
            if (std::scanf("%lld", &value) != 1) {
                return 1;
            }
            queue.push_back(value);
        } else if (kind == 2 && !queue.empty()) {
            std::printf("%lld\n", queue.front());
            queue.erase(queue.begin());
        } else {
            return 1;
        }
    }
    return 0;
}
