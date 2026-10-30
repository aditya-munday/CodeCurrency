#include "pair_count.h"

#include <stdio.h>
#include <stdlib.h>

int main(void) {
    long long count = 0;
    long long threshold = 0;
    if (scanf("%lld %lld", &count, &threshold) != 2 || count <= 0 || count > 200000) {
        return 1;
    }

    long long *values = malloc((size_t)count * sizeof(*values));
    if (values == NULL) {
        return 1;
    }
    for (long long index = 0; index < count; ++index) {
        if (scanf("%lld", &values[index]) != 1) {
            free(values);
            return 1;
        }
    }

    printf("%lld\n", count_pairs(values, (size_t)count, threshold));
    free(values);
    return 0;
}
