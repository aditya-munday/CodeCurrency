#include <cstdio>

static char input[8 * 1024 * 1024];

static long long next_integer(const char *&cursor, const char *end) {
    while (cursor < end && *cursor != '-' && (*cursor < '0' || *cursor > '9')) {
        cursor++;
    }
    long long sign = 1;
    if (cursor < end && *cursor == '-') {
        sign = -1;
        cursor++;
    }
    long long value = 0;
    while (cursor < end && *cursor >= '0' && *cursor <= '9') {
        value = value * 10 + (*cursor - '0');
        cursor++;
    }
    return sign * value;
}

int main() {
    size_t length = std::fread(input, 1, sizeof(input), stdin);
    const char *cursor = input;
    const char *end = input + length;
    long long n = next_integer(cursor, end);
    if (n < 1 || n > 500000) {
        return 1;
    }
    long long sum = 0;
    for (long long i = 0; i < n; i++) {
        sum += next_integer(cursor, end);
    }
    std::printf("%lld\n", sum);
    return 0;
}
