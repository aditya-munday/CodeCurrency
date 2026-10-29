# Hints

1. The values are already separated by commas. You do not need a general CSV
   quoting implementation.
2. A decimal integer can be accumulated one digit at a time after reading an
   optional minus sign.
3. Compare the cost of conversion with the cost of fetching each byte. A
   contiguous buffer removes repeated input calls.
