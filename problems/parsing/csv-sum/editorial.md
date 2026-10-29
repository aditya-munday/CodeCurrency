# Editorial

Formatted extraction is convenient and validates the input structure as it
reads each value and delimiter. Its stream sentry, locale, and formatting logic
add work for every field.

A small integer parser can skip delimiters, read an optional minus sign, and
accumulate decimal digits. Reading through `getchar_unlocked` avoids formatted
conversion but still makes a library call for every byte.

The buffered solution reads the input into one byte array and advances a
pointer through it. It performs the same decimal conversion while avoiding the
per-byte input call. All three methods are linear in input length, so this
problem demonstrates constant-factor differences in parsing and input access.
