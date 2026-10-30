package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
	"sort"
)

func nextInt(data []byte, cursor *int) (int64, bool) {
	for *cursor < len(data) && data[*cursor] <= ' ' {
		*cursor++
	}
	if *cursor >= len(data) {
		return 0, false
	}
	var sign int64 = 1
	if data[*cursor] == '-' {
		sign = -1
		*cursor++
	}
	var value int64
	for *cursor < len(data) && data[*cursor] >= '0' && data[*cursor] <= '9' {
		value = value*10 + int64(data[*cursor]-'0')
		*cursor++
	}
	return value * sign, true
}

func main() {
	data, err := io.ReadAll(os.Stdin)
	if err != nil {
		return
	}
	cursor := 0
	n64, ok := nextInt(data, &cursor)
	if !ok || n64 < 0 || n64 > 200000 {
		return
	}
	threshold, ok := nextInt(data, &cursor)
	if !ok {
		return
	}
	n := int(n64)
	values := make([]int64, n)
	for i := range values {
		values[i], ok = nextInt(data, &cursor)
		if !ok {
			return
		}
	}
	sort.Slice(values, func(i, j int) bool { return values[i] < values[j] })
	left, right := 0, n
	var pairs int64
	for left < right {
		if values[left]+values[right-1] <= threshold {
			pairs += int64(right - left - 1)
			left++
		} else {
			right--
		}
	}
	writer := bufio.NewWriter(os.Stdout)
	fmt.Fprintln(writer, pairs)
	writer.Flush()
}
