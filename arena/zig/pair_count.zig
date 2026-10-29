extern "c" fn qsort(base: [*]u8, count: usize, size: usize, compare: *const fn (?*const anyopaque, ?*const anyopaque) callconv(.c) c_int) void;

fn lessThan(left: ?*const anyopaque, right: ?*const anyopaque) callconv(.c) c_int {
    const a: *const i64 = @ptrCast(@alignCast(left.?));
    const b: *const i64 = @ptrCast(@alignCast(right.?));
    return if (a.* < b.*) -1 else if (a.* > b.*) 1 else 0;
}

pub fn count_pairs(values: []i64, threshold: i64) i64 {
    qsort(@ptrCast(values.ptr), values.len, @sizeOf(i64), lessThan);
    var left: usize = 0;
    var right = values.len;
    var pairs: i64 = 0;
    while (left < right) {
        if (values[left] + values[right - 1] <= threshold) {
            pairs += @intCast(right - left - 1);
            left += 1;
        } else {
            right -= 1;
        }
    }
    return pairs;
}
