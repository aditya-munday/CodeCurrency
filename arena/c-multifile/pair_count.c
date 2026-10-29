#include "pair_count.h"

#include <stdlib.h>

static int compare_values(const void *left, const void *right) {
    long long a = *(const long long *)left;
    long long b = *(const long long *)right;
    return (a > b) - (a < b);
}

long long count_pairs(long long *values, size_t count, long long threshold) {
    qsort(values, count, sizeof(*values), compare_values);
    size_t left = 0;
    size_t right = count;
    long long pairs = 0;
    while (left < right) {
        if (values[left] + values[right - 1] <= threshold) {
            pairs += (long long)(right - left - 1);
            ++left;
        } else {
            --right;
        }
    }
    return pairs;
}
