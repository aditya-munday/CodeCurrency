module pair_count;

import std.algorithm : sort;

long count_pairs(long[] values, long threshold) {
    values.sort;
    size_t left = 0;
    size_t right = values.length;
    long pairs = 0;
    while (left < right) {
        if (values[left] + values[right - 1] <= threshold) {
            pairs += cast(long)(right - left - 1);
            left++;
        } else {
            right--;
        }
    }
    return pairs;
}
