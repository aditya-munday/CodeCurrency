package main

import "sort"

func countPairs(values []int64, threshold int64) int64 {
	sort.Slice(values, func(left, right int) bool {
		return values[left] < values[right]
	})
	left := 0
	right := len(values)
	var pairs int64
	for left < right {
		if values[left]+values[right-1] <= threshold {
			pairs += int64(right - left - 1)
			left++
		} else {
			right--
		}
	}
	return pairs
}
