package main

import "core:sort"

count_pairs :: proc(values: []i64, threshold: i64) -> i64 {
	sort.quick_sort(values)
	left, right := 0, len(values)
	pairs: i64 = 0
	for left < right {
		if values[left]+values[right-1] <= threshold {
			pairs += i64(right-left-1)
			left += 1
		} else {
			right -= 1
		}
	}
	return pairs
}
