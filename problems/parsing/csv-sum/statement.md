# Sum a Delimited Integer Stream

The first line contains an integer `N`. The second line contains exactly `N`
comma-separated signed integers. Print their sum followed by a newline.

The input satisfies `1 <= N <= 500000`. Each value is between `-1000000` and
`1000000`, so the sum fits in a signed 64-bit integer. There is no whitespace
around the commas.

The experiment compares formatted stream extraction, a character-at-a-time
integer parser, and a buffered parser that scans contiguous bytes.
