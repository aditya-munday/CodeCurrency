#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        return 2;
    }
    unsigned long long seed = strtoull(argv[1], NULL, 10);
    long long n = strtoll(argv[2], NULL, 10);
    if (n < 2 || n > 120000) {
        return 2;
    }
    unsigned long long state = seed ? seed : 0x9E3779B97F4A7C15ULL;
    printf("%lld %lld\n", n, n / 2);
    for (long long i = 0; i < n; i++) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        unsigned long long value = state * 2685821657736338717ULL;
        long long item = (long long)(value % 2000000001ULL) - 1000000000LL;
        printf("%lld%c", item, i + 1 == n ? '\n' : ' ');
    }
    return 0;
}
