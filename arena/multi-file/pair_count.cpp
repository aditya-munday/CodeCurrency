#include "pair_count.hpp"

#include <algorithm>

long long count_pairs(std::vector<long long>& values, long long threshold) {
    std::sort(values.begin(), values.end());
    long long count = 0;
    long long left = 0;
    long long right = static_cast<long long>(values.size()) - 1;
    while (left < right) {
        if (values[static_cast<std::size_t>(left)] +
                values[static_cast<std::size_t>(right)] <= threshold) {
            count += right - left;
            ++left;
        } else {
            --right;
        }
    }
    return count;
}
