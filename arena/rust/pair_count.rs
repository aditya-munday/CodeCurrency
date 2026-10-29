pub fn count_pairs(values: &mut [i64], threshold: i64) -> u64 {
    values.sort_unstable();
    let mut left = 0usize;
    let mut right = values.len();
    let mut pairs = 0u64;
    while left < right {
        if values[left] + values[right - 1] <= threshold {
            pairs += (right - left - 1) as u64;
            left += 1;
        } else {
            right -= 1;
        }
    }
    pairs
}
