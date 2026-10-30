#include <cstdio>

static long long next_integer() {
    int ch = getchar_unlocked();
    while (ch != EOF && ch != '-' && (ch < '0' || ch > '9')) {
        ch = getchar_unlocked();
    }
    long long sign = 1;
    if (ch == '-') {
        sign = -1;
        ch = getchar_unlocked();
    }
    long long value = 0;
    while (ch >= '0' && ch <= '9') {
        value = value * 10 + (ch - '0');
        ch = getchar_unlocked();
    }
    return sign * value;
}

int main() {
    long long n = next_integer();
    if (n < 1 || n > 500000) {
        return 1;
    }
    long long sum = 0;
    for (long long i = 0; i < n; i++) {
        sum += next_integer();
    }
    std::printf("%lld\n", sum);
    return 0;
}
