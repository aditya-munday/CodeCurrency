#include <cstdio>

struct Node {
    long long value;
    Node *next;
};

int main() {
    int n = 0;
    if (std::scanf("%d", &n) != 1 || n < 1) {
        return 1;
    }
    Node *head = nullptr;
    Node *tail = nullptr;
    for (int i = 0; i < n; i++) {
        long long value = 0;
        if (std::scanf("%lld", &value) != 1) {
            return 1;
        }
        Node *node = new Node{value, nullptr};
        if (tail == nullptr) {
            head = node;
        } else {
            tail->next = node;
        }
        tail = node;
    }
    long long checksum = 0;
    long long position = 1;
    while (head != nullptr) {
        Node *next = head->next;
        checksum += position * head->value;
        delete head;
        head = next;
        position++;
    }
    std::printf("%lld\n", checksum);
    return 0;
}
