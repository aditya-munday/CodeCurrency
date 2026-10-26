// Great solution: counting sort, then a prefix sum. O(n + V).
//
// Values are bounded by 2n, so a counting pass beats a comparison sort. The
// qualifying-partner count for a value v is then a prefix sum up to T - v,
// which turns the pair count into a single linear scan with no inner loop and
// no data-dependent branches.
#include <cstdio>
#include <vector>

int main() {
    long long n = 0, threshold = 0;
    if (scanf("%lld %lld", &n, &threshold) != 2) {
        return 1;
    }
    std::vector<long long> a(static_cast<size_t>(n));
    long long maxv = 0;
    for (long long i = 0; i < n; i++) {
        if (scanf("%lld", &a[static_cast<size_t>(i)]) != 1) {
            return 1;
        }
        if (a[static_cast<size_t>(i)] > maxv) {
            maxv = a[static_cast<size_t>(i)];
        }
    }

    std::vector<long long> freq(static_cast<size_t>(maxv + 1), 0);
    for (long long i = 0; i < n; i++) {
        freq[static_cast<size_t>(a[static_cast<size_t>(i)])]++;
    }

    // prefix[v] = number of elements with value <= v.
    std::vector<long long> prefix(static_cast<size_t>(maxv + 1), 0);
    long long running = 0;
    for (long long v = 0; v <= maxv; v++) {
        running += freq[static_cast<size_t>(v)];
        prefix[static_cast<size_t>(v)] = running;
    }

    long long count = 0;
    for (long long v = 0; v <= maxv; v++) {
        long long f = freq[static_cast<size_t>(v)];
        if (f == 0) {
            continue;
        }
        // Pairs where both elements equal v: choose(f, 2) when 2v <= threshold.
        if (2 * v <= threshold) {
            count += f * (f - 1) / 2;
        }
        // Pairs with a strictly smaller partner u <= threshold - v.
        long long limit = threshold - v;
        if (limit > v - 1) {
            limit = v - 1;
        }
        if (limit >= 0) {
            if (limit > maxv) {
                limit = maxv;
            }
            count += f * prefix[static_cast<size_t>(limit)];
        }
    }
    printf("%lld\n", count);
    return 0;
}
