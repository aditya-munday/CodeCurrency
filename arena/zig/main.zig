extern "c" fn read(fd: c_int, buffer: [*]u8, count: usize) isize;
extern "c" fn write(fd: c_int, buffer: [*]const u8, count: usize) isize;

const pair_count = @import("pair_count.zig");

var input: [16 * 1024 * 1024]u8 = undefined;
var values: [200000]i64 = undefined;

fn nextInteger(bytes: []const u8, cursor: *usize) ?i64 {
    while (cursor.* < bytes.len and bytes[cursor.*] <= ' ') : (cursor.* += 1) {}
    if (cursor.* == bytes.len) return null;
    var sign: i64 = 1;
    if (bytes[cursor.*] == '-') {
        sign = -1;
        cursor.* += 1;
    }
    var value: i64 = 0;
    while (cursor.* < bytes.len and bytes[cursor.*] >= '0' and bytes[cursor.*] <= '9') : (cursor.* += 1) {
        value = value * 10 + bytes[cursor.*] - '0';
    }
    return value * sign;
}

pub fn main() void {
    const size = read(0, &input, input.len);
    if (size <= 0) return;
    const bytes = input[0..@intCast(size)];
    var cursor: usize = 0;
    const count: usize = @intCast(nextInteger(bytes, &cursor) orelse return);
    const threshold = nextInteger(bytes, &cursor) orelse return;
    if (count > values.len) return;
    for (0..count) |index| values[index] = nextInteger(bytes, &cursor) orelse return;
    const pairs = pair_count.count_pairs(values[0..count], threshold);
    var output: [32]u8 = undefined;
    var length: usize = 0;
    var number: u64 = @intCast(pairs);
    if (number == 0) {
        output[0] = '0';
        length = 1;
    } else {
        while (number > 0) : (number /= 10) {
            output[length] = @intCast('0' + number % 10);
            length += 1;
        }
        var index: usize = 0;
        while (index < length / 2) : (index += 1) {
            const temporary = output[index];
            output[index] = output[length - index - 1];
            output[length - index - 1] = temporary;
        }
    }
    output[length] = '\n';
    _ = write(1, &output, length + 1);
}
