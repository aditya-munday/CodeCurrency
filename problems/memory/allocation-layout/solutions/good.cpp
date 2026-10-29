#include <cstdio>
#include <vector>

struct Node {
    long long value;
    size_t next;
};

int main() {
    int n = 0;
    if (std::scanf("%d", &n) != 1 || n < 1) {
        return 1;
    }
    std::vector<Node> nodes;
    nodes.reserve(static_cast<size_t>(n));
    for (int i = 0; i < n; i++) {
        long long value = 0;
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
        nodes.push_back(Node{value, static_cast<size_t>(i + 1)});
    }
    long long checksum = 0;
    size_t index = 0;
    long long position = 1;
    while (index < nodes.size()) {
        const Node &node = nodes[index];
        checksum += position * node.value;
        index = node.next;
        position++;
    }
    std::printf("%lld\n", checksum);
    return 0;
}
