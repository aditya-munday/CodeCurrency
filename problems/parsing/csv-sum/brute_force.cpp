#include <cstdio>
#include <iostream>
#include <sstream>
#include <string>

int main() {
    int n = 0;
    if (!(std::cin >> n) || n < 1) {
        return 1;
    }
    std::string remainder;
    std::getline(std::cin, remainder);
    std::string row;
    if (!std::getline(std::cin, row)) {
        return 1;
    }
    std::stringstream fields(row);
    std::string token;
    long long sum = 0;
    int count = 0;
    while (std::getline(fields, token, ',')) {
        try {
            sum += std::stoll(token);
        } catch (...) {
            return 1;
        }
        count++;
    }
    if (count != n) {
        return 1;
    }
    std::printf("%lld\n", sum);
    return 0;
}
