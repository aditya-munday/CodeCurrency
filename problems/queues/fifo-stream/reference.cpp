#include <cstdio>
#include <vector>

int main() {
    int operations = 0;
    if (std::scanf("%d", &operations) != 1 || operations < 1) {
        return 1;
    }
    std::vector<long long> storage(static_cast<size_t>(operations));
    size_t head = 0;
    size_t tail = 0;
    size_t length = 0;
    for (int i = 0; i < operations; i++) {
        int kind = 0;
        if (std::scanf("%d", &kind) != 1) {
            return 1;
        }
        if (kind == 1) {
            long long value = 0;
            if (std::scanf("%lld", &value) != 1 || length == storage.size()) {
                return 1;
            }
            storage[tail] = value;
            tail++;
            if (tail == storage.size()) {
                tail = 0;
            }
            length++;
        } else if (kind == 2 && length > 0) {
            std::printf("%lld\n", storage[head]);
            head++;
            if (head == storage.size()) {
                head = 0;
            }
            length--;
        } else {
            return 1;
        }
    }
    return 0;
}
