import std.conv : to;
import std.stdio : stdin, writeln;
import std.string : split;
import pair_count : count_pairs;

void main() {
    string input;
    foreach (line; stdin.byLine) {
        input ~= line;
        input ~= ' ';
    }
    auto tokens = input.split;
    auto n = tokens[0].to!size_t;
    auto threshold = tokens[1].to!long;
    long[] values;
    values.length = n;
    foreach (i; 0 .. n) {
        values[i] = tokens[i + 2].to!long;
    }
    writeln(count_pairs(values, threshold));
}
