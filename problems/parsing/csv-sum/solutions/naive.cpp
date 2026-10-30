#include <iostream>

int main() {
    int n = 0;
    if (!(std::cin >> n) || n < 1) {
        return 1;
    }
    long long sum = 0;
    for (int i = 0; i < n; i++) {
        long long value = 0;
        if (!(std::cin >> value)) {
            return 1;
        }
        sum += value;
        if (i + 1 < n) {
            char comma = 0;
            if (!(std::cin >> comma) || comma != ',') {
                return 1;
            }
        }
    }
    std::cout << sum << '\n';
    return 0;
}
