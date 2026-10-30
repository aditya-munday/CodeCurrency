#include <stdio.h>
#include <stdlib.h>

int main(int argc, char **argv) {
    if (argc < 3) {
        return 2;
    }
    unsigned long long seed = strtoull(argv[1], NULL, 10);
    long long operations = strtoll(argv[2], NULL, 10);
    if (operations < 3 || operations > 200000) {
        return 2;
    }
    unsigned long long state = seed ? seed : 0x94D049BB133111EBULL;
    long long queued = 0;
    printf("%lld\n", operations);
    for (long long i = 0; i < operations; i++) {
        if (queued == 0 || i % 3 != 2) {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            unsigned long long value = state * 2685821657736338717ULL;
            long long item = (long long)(value % 2000001ULL) - 1000000LL;
            printf("1 %lld\n", item);
            queued++;
        } else {
            printf("2\n");
            queued--;
        }
    }
    return 0;
}
