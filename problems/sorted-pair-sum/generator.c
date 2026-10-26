/* Deterministic input generator: `gen SEED SCALE` writes one test case.
 *
 * The PRNG is xorshift64*, implemented here rather than taken from libc, so the
 * same seed produces the same bytes on every machine and every libc. A
 * generator that drifts would make every capsule unreplayable. */
#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        fprintf(stderr, "usage: gen SEED SCALE\n");
        return 2;
    }
    unsigned long long seed = strtoull(argv[1], NULL, 10);
    long long n = strtoll(argv[2], NULL, 10);
    if (n <= 0) {
        return 2;
    }

    unsigned long long s = seed ? seed : 0x9E3779B97F4A7C15ULL;
    /* A threshold that admits a meaningful fraction of pairs. */
    long long threshold = n;

    printf("%lld\n%lld\n", n, threshold);
    for (long long i = 0; i < n; i++) {
        s ^= s >> 12;
        s ^= s << 25;
        s ^= s >> 27;
        unsigned long long v = s * 2685821657736338717ULL;
        long long value = (long long)(v % (unsigned long long)(2 * n + 1));
        printf("%lld%c", value, i + 1 == n ? '\n' : ' ');
    }
    return 0;
}
