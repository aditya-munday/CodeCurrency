# Select the Largest K Values

Given `N`, `K`, and a sequence of `N` signed integers, print the largest `K`
values in descending order, one value per line. Equal values may appear more
than once because each input position contributes one value.

The input contains `1 <= K < N <= 120000`. Each value is between
`-1000000000` and `1000000000`.

The generated cases use `K = N / 2`. The output must contain exactly `K` lines.

The central experiment is how the choice of selection algorithm changes work
and storage: keep a sorted prefix, retain a bounded min-heap, or partition the
full input and sort only the selected range.
