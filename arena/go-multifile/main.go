package main

import (
	"bufio"
	"fmt"
	"io"
	"os"
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
	values := make([]int64, int(n64))
	for index := range values {
		values[index], ok = nextInt(data, &cursor)
		if !ok {
			return
		}
	}
	writer := bufio.NewWriter(os.Stdout)
	fmt.Fprintln(writer, countPairs(values, threshold))
	writer.Flush()
}
