package main

import "core:fmt"
import "core:os"

values: [200000]i64

parse_int :: proc(data: []u8, cursor: ^int) -> (value: i64, ok: bool) {
	for cursor^ < len(data) && data[cursor^] <= ' ' {
		cursor^ += 1
	}
	if cursor^ >= len(data) {
		return 0, false
	}
	sign: i64 = 1
	if data[cursor^] == '-' {
		sign = -1
		cursor^ += 1
	}
	for cursor^ < len(data) && data[cursor^] >= '0' && data[cursor^] <= '9' {
		value = value*10 + i64(data[cursor^]-'0')
		cursor^ += 1
	}
	return value*sign, true
}

main :: proc() {
	data, err := os.read_entire_file_from_file(os.stdin, context.temp_allocator)
	if err != nil {
		return
	}
	cursor := 0
	count64, ok := parse_int(data, &cursor)
	if !ok || count64 < 0 || count64 > 200000 {
		return
	}
	threshold, threshold_ok := parse_int(data, &cursor)
	if !threshold_ok {
		return
	}
	count := int(count64)
	for i in 0..<count {
		values[i], ok = parse_int(data, &cursor)
		if !ok {
			return
		}
	}
	fmt.println(count_pairs(values[:count], threshold))
}
