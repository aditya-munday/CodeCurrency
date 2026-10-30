#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <limits.h>
#include <linux/memfd.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#define MAX_BATCH_CASES 10000
#define MAX_COMMAND_ARGS 256
#define GENERATOR_OUTPUT_BYTES (256ULL * 1024ULL * 1024ULL)

struct buffer {
    unsigned char *data;
    size_t len;
    size_t cap;
};

struct run_result {
    int runner_status;
    unsigned char *output;
    size_t output_len;
    char verdict[2048];
};

struct options {
    uint64_t first_seed;
    uint64_t count;
    uint64_t scale;
    uint64_t timeout_ms;
    uint64_t memory_bytes;
    uint64_t solution_memory_bytes;
    uint64_t stack_bytes;
    uint64_t pids_limit;
    uint64_t max_output_bytes;
    const char *generator;
    const char *reference;
    const char *checker;
    char **solution_command;
    size_t solution_command_count;
};

static int append(struct buffer *buffer, const unsigned char *data, size_t len) {
    if (len > SIZE_MAX - buffer->len) {
        return -1;
    }
    size_t required = buffer->len + len;
    if (required > buffer->cap) {
        size_t cap = buffer->cap ? buffer->cap : 65536;
        while (cap < required) {
            if (cap > SIZE_MAX / 2) {
                cap = required;
                break;
            }
            cap *= 2;
        }
        unsigned char *next = realloc(buffer->data, cap);
        if (!next) {
            return -1;
        }
        buffer->data = next;
        buffer->cap = cap;
    }
    memcpy(buffer->data + buffer->len, data, len);
    buffer->len = required;
    return 0;
}

static int read_all(int fd, size_t limit, struct buffer *buffer) {
    unsigned char data[65536];
    for (;;) {
        ssize_t len = read(fd, data, sizeof data);
        if (len == 0) {
            return 0;
        }
        if (len < 0) {
            if (errno == EINTR) {
                continue;
            }
            return -1;
        }
        if ((size_t)len > limit - buffer->len || append(buffer, data, (size_t)len) != 0) {
            return -1;
        }
    }
}

static int read_line(const unsigned char *data, size_t len, size_t *position, char *line, size_t cap) {
    if (*position >= len) {
        return -1;
    }
    size_t start = *position;
    while (*position < len && data[*position] != '\n') {
        (*position)++;
    }
    if (*position >= len || *position - start >= cap) {
        return -1;
    }
    size_t line_len = *position - start;
    memcpy(line, data + start, line_len);
    line[line_len] = '\0';
    (*position)++;
    return 0;
}

static int read_section(const unsigned char *data, size_t len, size_t *position, const char *tag, const unsigned char **body, size_t *body_len) {
    char line[128];
    if (read_line(data, len, position, line, sizeof line) != 0) {
        return -1;
    }
    size_t tag_len = strlen(tag);
    if (strncmp(line, tag, tag_len) != 0) {
        return -1;
    }
    char *end = NULL;
    unsigned long long amount = strtoull(line + tag_len, &end, 10);
    if (!end || end == line + tag_len || *end != '\0' || amount > SIZE_MAX || amount > len - *position) {
        return -1;
    }
    *body = data + *position;
    *body_len = (size_t)amount;
    *position += (size_t)amount;
    return 0;
}

static int parse_runner_report(struct buffer *raw, struct run_result *result) {
    size_t position = 0;
    const unsigned char *out = NULL;
    const unsigned char *err = NULL;
    const unsigned char *alloc = NULL;
    const unsigned char *emit = NULL;
    size_t out_len = 0;
    size_t err_len = 0;
    size_t alloc_len = 0;
    size_t emit_len = 0;
    if (read_section(raw->data, raw->len, &position, "CCX-OUT ", &out, &out_len) != 0 ||
        read_section(raw->data, raw->len, &position, "CCX-ERR ", &err, &err_len) != 0 ||
        read_section(raw->data, raw->len, &position, "CCX-ALLOC ", &alloc, &alloc_len) != 0 ||
        read_section(raw->data, raw->len, &position, "CCX-EMIT ", &emit, &emit_len) != 0) {
        return -1;
    }
    char line[sizeof result->verdict];
    if (read_line(raw->data, raw->len, &position, line, sizeof line) != 0 ||
        strncmp(line, "CCX-VERDICT ", 12) != 0) {
        return -1;
    }
    const char *json = line + 12;
    if (strlen(json) >= sizeof result->verdict) {
        return -1;
    }
    strcpy(result->verdict, json);
    result->output = malloc(out_len ? out_len : 1);
    if (!result->output) {
        return -1;
    }
    if (out_len) {
        memcpy(result->output, out, out_len);
    }
    result->output_len = out_len;
    return 0;
}

static int wait_status_code(int status) {
    if (WIFEXITED(status)) {
        return WEXITSTATUS(status);
    }
    if (WIFSIGNALED(status)) {
        return 128 + WTERMSIG(status);
    }
    return 255;
}

static int invoke_runner(const struct options *options, char *const program_argv[], const char *stdin_path, uint64_t memory_limit, uint64_t output_limit, struct run_result *result) {
    char timeout_text[32];
    char memory_text[32];
    char stack_text[32];
    char pids_text[32];
    char output_text[32];
    snprintf(timeout_text, sizeof timeout_text, "%" PRIu64, options->timeout_ms);
    snprintf(memory_text, sizeof memory_text, "%" PRIu64, memory_limit);
    snprintf(stack_text, sizeof stack_text, "%" PRIu64, options->stack_bytes);
    snprintf(pids_text, sizeof pids_text, "%" PRIu64, options->pids_limit);
    snprintf(output_text, sizeof output_text, "%" PRIu64, output_limit);

    size_t program_argc = 0;
    while (program_argv[program_argc]) {
        program_argc++;
    }
    char **runner_argv = calloc(program_argc + 16, sizeof(char *));
    if (!runner_argv) {
        return -1;
    }
    size_t argc = 0;
    runner_argv[argc++] = "/opt/ccx/runner";
    runner_argv[argc++] = "--timeout-ms";
    runner_argv[argc++] = timeout_text;
    runner_argv[argc++] = "--memory-bytes";
    runner_argv[argc++] = memory_text;
    runner_argv[argc++] = "--stack-bytes";
    runner_argv[argc++] = stack_text;
    runner_argv[argc++] = "--pids-limit";
    runner_argv[argc++] = pids_text;
    runner_argv[argc++] = "--max-output-bytes";
    runner_argv[argc++] = output_text;
    if (stdin_path) {
        runner_argv[argc++] = "--stdin";
        runner_argv[argc++] = (char *)stdin_path;
    }
    runner_argv[argc++] = "--";
    for (size_t i = 0; i < program_argc; i++) {
        runner_argv[argc++] = program_argv[i];
    }

    int pipe_fds[2];
    if (pipe(pipe_fds) != 0) {
        free(runner_argv);
        return -1;
    }
    pid_t pid = fork();
    if (pid < 0) {
        close(pipe_fds[0]);
        close(pipe_fds[1]);
        free(runner_argv);
        return -1;
    }
    if (pid == 0) {
        int null_fd = open("/dev/null", O_WRONLY);
        close(pipe_fds[0]);
        dup2(pipe_fds[1], STDOUT_FILENO);
        if (null_fd >= 0) {
            dup2(null_fd, STDERR_FILENO);
        }
        close(pipe_fds[1]);
        if (null_fd > STDERR_FILENO) {
            close(null_fd);
        }
        execv("/opt/ccx/runner", runner_argv);
        _exit(127);
    }
    close(pipe_fds[1]);
    struct buffer raw = {0};
    size_t raw_limit = output_limit + 8192;
    int read_status = read_all(pipe_fds[0], raw_limit, &raw);
    close(pipe_fds[0]);
    int status = 0;
    while (waitpid(pid, &status, 0) < 0) {
        if (errno != EINTR) {
            read_status = -1;
            break;
        }
    }
    result->runner_status = wait_status_code(status);
    int parsed = read_status == 0 ? parse_runner_report(&raw, result) : -1;
    free(raw.data);
    free(runner_argv);
    return parsed;
}

static int json_number(const char *json, const char *name, long long *value) {
    char key[96];
    if (snprintf(key, sizeof key, "\"%s\":", name) >= (int)sizeof key) {
        return -1;
    }
    const char *field = strstr(json, key);
    if (!field) {
        return -1;
    }
    char *end = NULL;
    long long parsed = strtoll(field + strlen(key), &end, 10);
    if (end == field + strlen(key)) {
        return -1;
    }
    *value = parsed;
    return 0;
}

static int run_ok(const struct run_result *result) {
    long long exit_status = -1;
    long long signal_number = -1;
    if (json_number(result->verdict, "exit_status", &exit_status) != 0 ||
        json_number(result->verdict, "signal", &signal_number) != 0) {
        return 0;
    }
    return exit_status == 0 && signal_number == 0 &&
           strstr(result->verdict, "\"timed_out\":false") &&
           strstr(result->verdict, "\"output_truncated\":false");
}

static int make_blob(const unsigned char *data, size_t len) {
    int fd = (int)syscall(SYS_memfd_create, "ccx-batch-case", MFD_ALLOW_SEALING);
    if (fd < 0) {
        return -1;
    }
    size_t position = 0;
    while (position < len) {
        ssize_t written = write(fd, data + position, len - position);
        if (written < 0) {
            if (errno == EINTR) {
                continue;
            }
            close(fd);
            return -1;
        }
        position += (size_t)written;
    }
    if (fcntl(fd, F_ADD_SEALS, F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL) != 0 ||
        lseek(fd, 0, SEEK_SET) < 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int duplicate_high(int fd, int minimum) {
    int copy = fcntl(fd, F_DUPFD, minimum);
    if (copy < 0) {
        return -1;
    }
    if (fcntl(copy, F_SETFD, 0) != 0) {
        close(copy);
        return -1;
    }
    return copy;
}

static void free_result(struct run_result *result) {
    free(result->output);
    result->output = NULL;
    result->output_len = 0;
}

static void emit_record(uint64_t seed, const struct run_result *generator, const struct run_result *reference, const struct run_result *solution, const struct run_result *checker, int checker_present, int checker_accepted, size_t expected_len, size_t actual_len, int outputs_equal, size_t difference_offset, int stage_error) {
    printf("{\"seed\":%" PRIu64 ",\"stage_error\":", seed);
    if (stage_error == 1) {
        printf("\"generator\"");
    } else if (stage_error == 2) {
        printf("\"reference\"");
    } else {
        printf("null");
    }
    printf(",\"generator\":%s", generator->verdict[0] ? generator->verdict : "null");
    printf(",\"reference\":%s", reference->verdict[0] ? reference->verdict : "null");
    printf(",\"solution\":%s", solution->verdict[0] ? solution->verdict : "null");
    printf(",\"checker\":%s", checker_present && checker->verdict[0] ? checker->verdict : "null");
    printf(",\"checker_accepted\":%s,\"expected_bytes\":%zu,\"actual_bytes\":%zu,\"outputs_equal\":%s,\"difference_offset\":", checker_accepted ? "true" : "false", expected_len, actual_len, outputs_equal ? "true" : "false");
    if (outputs_equal) {
        printf("null");
    } else {
        printf("%zu", difference_offset);
    }
    printf("}\n");
    fflush(stdout);
}

static int parse_u64(const char *value, uint64_t *out) {
    if (!value || !*value || *value == '-') {
        return -1;
    }
    char *end = NULL;
    errno = 0;
    unsigned long long parsed = strtoull(value, &end, 10);
    if (errno || !end || *end != '\0') {
        return -1;
    }
    *out = (uint64_t)parsed;
    return 0;
}

static int parse_options(int argc, char **argv, struct options *options) {
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--first-seed") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->first_seed) != 0) return -1;
        } else if (strcmp(argv[i], "--count") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->count) != 0) return -1;
        } else if (strcmp(argv[i], "--scale") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->scale) != 0) return -1;
        } else if (strcmp(argv[i], "--timeout-ms") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->timeout_ms) != 0) return -1;
        } else if (strcmp(argv[i], "--memory-bytes") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->memory_bytes) != 0) return -1;
        } else if (strcmp(argv[i], "--solution-memory-bytes") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->solution_memory_bytes) != 0) return -1;
        } else if (strcmp(argv[i], "--stack-bytes") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->stack_bytes) != 0) return -1;
        } else if (strcmp(argv[i], "--pids-limit") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->pids_limit) != 0) return -1;
        } else if (strcmp(argv[i], "--max-output-bytes") == 0 && i + 1 < argc) {
            if (parse_u64(argv[++i], &options->max_output_bytes) != 0) return -1;
        } else if (strcmp(argv[i], "--generator") == 0 && i + 1 < argc) {
            options->generator = argv[++i];
        } else if (strcmp(argv[i], "--reference") == 0 && i + 1 < argc) {
            options->reference = argv[++i];
        } else if (strcmp(argv[i], "--checker") == 0 && i + 1 < argc) {
            options->checker = argv[++i];
        } else if (strcmp(argv[i], "--solution-command-count") == 0 && i + 1 < argc) {
            uint64_t count = 0;
            if (parse_u64(argv[++i], &count) != 0 || count > MAX_COMMAND_ARGS || (uint64_t)(argc - i - 1) < count) return -1;
            options->solution_command_count = (size_t)count;
            options->solution_command = &argv[i + 1];
            i += (int)count;
        } else {
            return -1;
        }
    }
    if (!options->generator || !options->reference || !options->checker ||
        !options->solution_command || options->solution_command_count == 0 ||
        options->count == 0 || options->count > MAX_BATCH_CASES ||
        options->timeout_ms == 0 || options->memory_bytes == 0 ||
        options->solution_memory_bytes == 0 || options->stack_bytes == 0 ||
        options->pids_limit == 0 || options->max_output_bytes == 0 ||
        options->first_seed > UINT64_MAX - (options->count - 1) ||
        options->scale > LLONG_MAX) {
        return -1;
    }
    return 0;
}

static int process_case(const struct options *options, uint64_t seed) {
    struct run_result generator = {0};
    struct run_result reference = {0};
    struct run_result solution = {0};
    struct run_result checker = {0};
    char seed_text[32];
    char scale_text[32];
    snprintf(seed_text, sizeof seed_text, "%" PRIu64, seed);
    snprintf(scale_text, sizeof scale_text, "%" PRIu64, options->scale);
    char *generator_argv[] = {(char *)options->generator, seed_text, scale_text, NULL};
    if (invoke_runner(options, generator_argv, NULL, options->memory_bytes, GENERATOR_OUTPUT_BYTES, &generator) != 0) {
        fprintf(stderr, "ccx-batch-runner: generator report was invalid\n");
        return 2;
    }
    if (!run_ok(&generator)) {
        emit_record(seed, &generator, &reference, &solution, &checker, 0, 0, 0, 0, 0, 0, 1);
        free_result(&generator);
        return 3;
    }

    int input_fd = make_blob(generator.output, generator.output_len);
    if (input_fd < 0) {
        fprintf(stderr, "ccx-batch-runner: sealed input buffer is unavailable\n");
        free_result(&generator);
        return 2;
    }
    char input_path[64];
    snprintf(input_path, sizeof input_path, "/proc/self/fd/%d", input_fd);

    char *reference_argv[] = {(char *)options->reference, NULL};
    if (invoke_runner(options, reference_argv, input_path, options->memory_bytes, options->max_output_bytes, &reference) != 0) {
        fprintf(stderr, "ccx-batch-runner: reference report was invalid\n");
        close(input_fd);
        free_result(&generator);
        return 2;
    }
    if (!run_ok(&reference)) {
        emit_record(seed, &generator, &reference, &solution, &checker, 0, 0, 0, 0, 0, 0, 2);
        close(input_fd);
        free_result(&generator);
        free_result(&reference);
        return 3;
    }

    size_t solution_argc = options->solution_command_count;
    char **solution_argv = calloc(solution_argc + 1, sizeof(char *));
    if (!solution_argv) {
        close(input_fd);
        free_result(&generator);
        free_result(&reference);
        return 2;
    }
    for (size_t i = 0; i < options->solution_command_count; i++) {
        solution_argv[i] = options->solution_command[i];
    }
    solution_argv[solution_argc] = NULL;

    if (invoke_runner(options, solution_argv, input_path, options->solution_memory_bytes, options->max_output_bytes, &solution) != 0) {
        fprintf(stderr, "ccx-batch-runner: solution report was invalid\n");
        free(solution_argv);
        close(input_fd);
        free_result(&generator);
        free_result(&reference);
        return 2;
    }
    free(solution_argv);

    int outputs_equal = solution.output_len == reference.output_len &&
                        memcmp(solution.output, reference.output, solution.output_len) == 0;
    size_t difference_offset = 0;
    if (!outputs_equal) {
        size_t common = solution.output_len < reference.output_len ? solution.output_len : reference.output_len;
        while (difference_offset < common && solution.output[difference_offset] == reference.output[difference_offset]) {
            difference_offset++;
        }
    }

    int checker_present = options->checker && strcmp(options->checker, "-") != 0;
    int checker_accepted = outputs_equal;
    if (checker_present && run_ok(&solution)) {
        int expected_fd = make_blob(reference.output, reference.output_len);
        int actual_fd = make_blob(solution.output, solution.output_len);
        int input_arg_fd = duplicate_high(input_fd, 100);
        int expected_arg_fd = expected_fd >= 0 ? duplicate_high(expected_fd, 110) : -1;
        int actual_arg_fd = actual_fd >= 0 ? duplicate_high(actual_fd, 120) : -1;
        if (input_arg_fd < 0 || expected_arg_fd < 0 || actual_arg_fd < 0) {
            fprintf(stderr, "ccx-batch-runner: sealed checker inputs are unavailable\n");
            if (input_arg_fd >= 0) close(input_arg_fd);
            if (expected_arg_fd >= 0) close(expected_arg_fd);
            if (actual_arg_fd >= 0) close(actual_arg_fd);
            if (expected_fd >= 0) close(expected_fd);
            if (actual_fd >= 0) close(actual_fd);
            close(input_fd);
            free_result(&generator);
            free_result(&reference);
            free_result(&solution);
            return 2;
        }
        char input_arg[64];
        char expected_arg[64];
        char actual_arg[64];
        snprintf(input_arg, sizeof input_arg, "/proc/self/fd/%d", input_arg_fd);
        snprintf(expected_arg, sizeof expected_arg, "/proc/self/fd/%d", expected_arg_fd);
        snprintf(actual_arg, sizeof actual_arg, "/proc/self/fd/%d", actual_arg_fd);
        char *checker_argv[] = {(char *)options->checker, input_arg, expected_arg, actual_arg, NULL};
        if (invoke_runner(options, checker_argv, NULL, options->memory_bytes, options->max_output_bytes, &checker) != 0) {
            fprintf(stderr, "ccx-batch-runner: checker report was invalid\n");
            close(input_arg_fd);
            close(expected_arg_fd);
            close(actual_arg_fd);
            close(expected_fd);
            close(actual_fd);
            close(input_fd);
            free_result(&generator);
            free_result(&reference);
            free_result(&solution);
            return 2;
        }
        checker_accepted = run_ok(&checker);
        close(input_arg_fd);
        close(expected_arg_fd);
        close(actual_arg_fd);
        close(expected_fd);
        close(actual_fd);
    }

    emit_record(seed, &generator, &reference, &solution, &checker, checker_present, checker_accepted,
                reference.output_len, solution.output_len, outputs_equal, difference_offset, 0);
    close(input_fd);
    free_result(&generator);
    free_result(&reference);
    free_result(&solution);
    free_result(&checker);
    if (!run_ok(&solution)) {
        return 4;
    }
    if (checker_present ? !checker_accepted : !outputs_equal) {
        return 4;
    }
    return 0;
}

int main(int argc, char **argv) {
    setvbuf(stdout, NULL, _IOLBF, 0);
    struct options options = {0};
    if (parse_options(argc, argv, &options) != 0) {
        fprintf(stderr, "ccx-batch-runner: invalid options\n");
        return 2;
    }
    for (uint64_t index = 0; index < options.count; index++) {
        int status = process_case(&options, options.first_seed + index);
        if (status == 4) {
            return 0;
        }
        if (status != 0) {
            return status;
        }
    }
    return 0;
}
