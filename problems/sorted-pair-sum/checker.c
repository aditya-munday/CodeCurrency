#include <stdio.h>

int main(int argc, char **argv) {
    if (argc != 4) {
        return 1;
    }
    FILE *expected_file = fopen(argv[2], "r");
    FILE *actual_file = fopen(argv[3], "r");
    if (expected_file == NULL || actual_file == NULL) {
        if (expected_file != NULL) {
            fclose(expected_file);
        }
        if (actual_file != NULL) {
            fclose(actual_file);
        }
        return 1;
    }
    long long expected = 0;
    long long actual = 0;
    int expected_ok = fscanf(expected_file, "%lld", &expected) == 1;
    int actual_ok = fscanf(actual_file, "%lld", &actual) == 1;
    fclose(expected_file);
    fclose(actual_file);
    return expected_ok && actual_ok && expected == actual ? 0 : 1;
}
