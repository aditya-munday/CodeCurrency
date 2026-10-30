#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        return 2;
    }
    unsigned long long seed = strtoull(argv[1], NULL, 10);
    long long n = strtoll(argv[2], NULL, 10);
    if (n < 1 || n > 200000) {
        return 2;
    }
    unsigned long long state = seed ? seed : 0xBF58476D1CE4E5B9ULL;
    printf("%lld\n", n);
    for (long long i = 0; i < n; i++) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        unsigned long long value = state * 2685821657736338717ULL;
        long long item = (long long)(value % 2001ULL) - 1000LL;
        printf("%lld%c", item, i + 1 == n ? '\n' : ' ');
    }
    return 0;
}
