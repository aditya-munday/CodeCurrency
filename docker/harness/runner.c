/*
 * ccx-runner — the measurement runner that lives inside every toolchain image.
 *
 * It is the only thing that executes a submission. Because the submission is
 * untrusted, the runner owns the containment: rlimits, a wall-clock timeout,
 * process-group kill, and a hard cap on captured output.
 *
 * Peak memory comes from wait4(2) rusage of the child, which is the honest
 * number for "how much memory did this solution actually touch", rather than
 * the container's own peak.
 *
 * OUTPUT PROTOCOL
 *
 * The child's stdout and stderr are redirected into pipes. The runner then
 * writes a framed report on its *own* stdout, which is the container's stdout
 * and therefore reaches the host. The child cannot inject into that stream,
 * because fd 1 in the child is the pipe, not the container's stdout. That is
 * what makes the report tamper-proof without needing any writable host mount:
 *
 *     CCX-OUT <byte-count>\n
 *     <that many raw bytes of child stdout>
 *     CCX-ERR <byte-count>\n
 *     <that many raw bytes of child stderr>
 *     CCX-VERDICT <json>\n
 *
 * Byte counts make the framing binary-safe. The harness parses this stream and
 * never has to guess where child output ends and the verdict begins.
 *
 * Exit codes: 0..125 = child status (128+n for a signaled child),
 *             124 = timed out, 125 = output truncated, 2 = runner misuse.
 *
 * Usage:
 *   ccx-runner --timeout-ms N --memory-bytes N --stack-bytes N \
 *              --pids-limit N --max-output-bytes N \
 *              -- prog args...
 */

#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/perf_event.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define EXIT_TIMEOUT 124
#define EXIT_OUTPUT_TRUNCATED 125
#define EXIT_NOT_EXECUTABLE 126
#define EXIT_MISUSE 2
#define MAX_EMIT_BYTES (64LL * 1024 * 1024)

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

static long long timeval_ns(struct timeval value) {
    return (long long)value.tv_sec * 1000000000LL + (long long)value.tv_usec * 1000LL;
}

struct hardware_fds {
    int instructions;
    int cache_misses;
    int branch_misses;
    int cpu_cycles;
};

static int open_hardware_counter(pid_t pid, uint64_t event) {
#ifdef SYS_perf_event_open
    struct perf_event_attr attr;
    memset(&attr, 0, sizeof attr);
    attr.type = PERF_TYPE_HARDWARE;
    attr.size = sizeof attr;
    attr.config = event;
    attr.disabled = 1;
    attr.inherit = 1;
    attr.inherit_stat = 1;
    attr.read_format = PERF_FORMAT_TOTAL_TIME_ENABLED | PERF_FORMAT_TOTAL_TIME_RUNNING;
    attr.exclude_kernel = 1;
    attr.exclude_hv = 1;
    int fd = (int)syscall(SYS_perf_event_open, &attr, pid, -1, -1, 0);
    if (fd < 0) {
        return -1;
    }
    if (ioctl(fd, PERF_EVENT_IOC_RESET, 0) != 0 || ioctl(fd, PERF_EVENT_IOC_ENABLE, 0) != 0) {
        close(fd);
        return -1;
    }
    return fd;
#else
    (void)pid;
    (void)event;
    return -1;
#endif
}

static long long close_hardware_counter(int fd) {
    if (fd < 0) {
        return -1;
    }
    ioctl(fd, PERF_EVENT_IOC_DISABLE, 0);
    struct {
        uint64_t value;
        uint64_t time_enabled;
        uint64_t time_running;
    } result = {0, 0, 0};
    ssize_t amount = read(fd, &result, sizeof result);
    close(fd);
    if (amount != (ssize_t)sizeof result || result.time_running == 0) {
        return -1;
    }
    __uint128_t scaled = (__uint128_t)result.value * result.time_enabled / result.time_running;
    return scaled > 0x7fffffffffffffffULL ? -1 : (long long)scaled;
}

/* A bounded byte buffer for captured child output. */
struct capture {
    char *data;
    size_t len;
    size_t cap;
    int truncated;
};

static int capture_init(struct capture *c) {
    c->data = malloc(1);
    if (!c->data) {
        return -1;
    }
    c->len = 0;
    c->cap = 0;
    c->truncated = 0;
    return 0;
}

static void capture_append(struct capture *c, const char *buf, size_t n, size_t available) {
    size_t take = n < available ? n : available;
    size_t required = c->len + take;
    if (required > c->cap) {
        size_t capacity = c->cap ? c->cap : 4096;
        while (capacity < required) {
            if (capacity > SIZE_MAX / 2) {
                capacity = required;
                break;
            }
            capacity *= 2;
        }
        char *expanded = realloc(c->data, capacity + 1);
        if (!expanded) {
            c->truncated = 1;
            return;
        }
        c->data = expanded;
        c->cap = capacity;
    }
    memcpy(c->data + c->len, buf, take);
    c->len += take;
    if (take < n) {
        c->truncated = 1;
    }
}

static int terminate_descendants(void) {
    char path[128];
    int path_len = snprintf(path, sizeof path, "/proc/self/task/%ld/children", (long)getpid());
    if (path_len < 0 || (size_t)path_len >= sizeof path) {
        return -1;
    }
    int found = 0;
    for (int attempt = 0; attempt < 128; attempt++) {
        FILE *children_file = fopen(path, "r");
        if (!children_file) {
            return -1;
        }
        pid_t children[128];
        size_t count = 0;
        long child_pid;
        while (fscanf(children_file, "%ld", &child_pid) == 1) {
            if (child_pid <= 0 || count == sizeof children / sizeof children[0]) {
                fclose(children_file);
                return -1;
            }
            children[count++] = (pid_t)child_pid;
        }
        fclose(children_file);
        if (count == 0) {
            int status = 0;
            pid_t waited = waitpid(-1, &status, WNOHANG);
            if (waited > 0) {
                found = 1;
                continue;
            }
            if (waited < 0 && errno == ECHILD) {
                return found;
            }
            if (waited < 0 && errno != EINTR) {
                return -1;
            }
            struct timespec delay = {0, 1000000};
            nanosleep(&delay, NULL);
            continue;
        }
        found = 1;
        for (size_t index = 0; index < count; index++) {
            if (kill(children[index], SIGKILL) != 0 && errno != ESRCH) {
                return -1;
            }
        }
        for (size_t index = 0; index < count; index++) {
            int status = 0;
            while (waitpid(children[index], &status, 0) < 0) {
                if (errno == EINTR) {
                    continue;
                }
                if (errno != ECHILD) {
                    return -1;
                }
                break;
            }
        }
    }
    return -1;
}

static size_t read_alloc_xtree(const char *path, pid_t process, char *out, size_t out_size) {
    if (!path || !out || out_size == 0) {
        return 0;
    }
    char expanded_path[4096];
    const char *marker = strstr(path, "%p");
    if (marker) {
        char process_text[32];
        int process_len = snprintf(process_text, sizeof process_text, "%ld", (long)process);
        size_t prefix_len = (size_t)(marker - path);
        size_t suffix_len = strlen(marker + 2);
        if (process_len < 0 || (size_t)process_len >= sizeof process_text
            || prefix_len + (size_t)process_len + suffix_len >= sizeof expanded_path) {
            return 0;
        }
        memcpy(expanded_path, path, prefix_len);
        memcpy(expanded_path + prefix_len, process_text, (size_t)process_len);
        memcpy(expanded_path + prefix_len + (size_t)process_len, marker + 2, suffix_len + 1);
        path = expanded_path;
    }
    int fd = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
    if (fd < 0) {
        return 0;
    }
    struct stat file_stat;
    if (fstat(fd, &file_stat) != 0 || !S_ISREG(file_stat.st_mode) || file_stat.st_size < 0 || file_stat.st_size > MAX_EMIT_BYTES) {
        close(fd);
        return 0;
    }
    FILE *file = fdopen(fd, "r");
    if (!file) {
        close(fd);
        return 0;
    }

    int has_creator = 0;
    int has_events = 0;
    int has_totals = 0;
    unsigned long long totals[6] = {0};
    char line[4096];
    while (fgets(line, sizeof line, file)) {
        if (strcmp(line, "creator: xtree-1\n") == 0 || strcmp(line, "creator: xtree-1") == 0) {
            has_creator = 1;
        }
        char event_names[6][16] = {{0}};
        int event_consumed = 0;
        int event_count = sscanf(line, "events: %15s %15s %15s %15s %15s %15s%n",
                                 event_names[0], event_names[1], event_names[2],
                                 event_names[3], event_names[4], event_names[5],
                                 &event_consumed);
        if (event_count == 6
            && strcmp(event_names[0], "curB") == 0
            && strcmp(event_names[1], "curBk") == 0
            && strcmp(event_names[2], "totB") == 0
            && strcmp(event_names[3], "totBk") == 0
            && strcmp(event_names[4], "totFdB") == 0
            && strcmp(event_names[5], "totFdBk") == 0) {
            const char *tail = line + event_consumed;
            while (*tail == ' ' || *tail == '\t' || *tail == '\r' || *tail == '\n') {
                tail++;
            }
            if (*tail == '\0') {
                has_events = 1;
            }
        }
        if (strncmp(line, "totals:", 7) == 0) {
            if (has_totals) {
                fclose(file);
                return 0;
            }
            const char *value = line + 7;
            while (*value == ' ' || *value == '\t' || *value == '\r' || *value == '\n') {
                value++;
            }
            if (strncmp(value, "(null)", 6) == 0) {
                const char *tail = value + 6;
                while (*tail == ' ' || *tail == '\t' || *tail == '\r' || *tail == '\n') {
                    tail++;
                }
                if (*tail != '\0') {
                    fclose(file);
                    return 0;
                }
                has_totals = 1;
                continue;
            }
            int consumed = 0;
            int converted = sscanf(line, "totals: %llu %llu %llu %llu %llu %llu%n",
                                   &totals[0], &totals[1], &totals[2], &totals[3],
                                   &totals[4], &totals[5], &consumed);
            if (converted != 6) {
                fclose(file);
                return 0;
            }
            const char *tail = line + consumed;
            while (*tail == ' ' || *tail == '\t' || *tail == '\r' || *tail == '\n') {
                tail++;
            }
            if (*tail != '\0') {
                fclose(file);
                return 0;
            }
            has_totals = 1;
        }
    }
    fclose(file);
    if (!has_creator || !has_events || !has_totals) {
        return 0;
    }
    int written = snprintf(out, out_size,
                           "{\"allocations\":%llu,\"allocated_bytes\":%llu,\"frees\":%llu}",
                           totals[3], totals[2], totals[5]);
    if (written < 0 || (size_t)written >= out_size) {
        return 0;
    }
    return (size_t)written;
}

/* Set every limit that keeps a hostile submission from escaping. */
static int apply_limit(int resource, long long amount, const char *name) {
    struct rlimit rl;
    rl.rlim_cur = rl.rlim_max = (rlim_t)amount;
    if (setrlimit(resource, &rl) != 0) {
        int failure = errno;
        dprintf(STDERR_FILENO, "ccx-runner: cannot apply %s limit: %s\n", name, strerror(failure));
        return -1;
    }
    return 0;
}

static int apply_limits(long long memory_bytes, long long stack_bytes, int pids_limit) {
    if (apply_limit(RLIMIT_AS, memory_bytes, "address-space") != 0) {
        return -1;
    }

    if (apply_limit(RLIMIT_STACK, stack_bytes, "stack") != 0) {
        return -1;
    }

    if (apply_limit(RLIMIT_NPROC, pids_limit, "process-count") != 0) {
        return -1;
    }

    /* No core dumps: they would be written into the read-only rootfs and could
       leak memory contents. */
    if (apply_limit(RLIMIT_CORE, 0, "core-dump") != 0) {
        return -1;
    }
    return 0;
}

static int parse_integer(const char *text, long long minimum, long long maximum, long long *value) {
    if (!text || text[0] < '0' || text[0] > '9') {
        return -1;
    }
    errno = 0;
    char *end = NULL;
    long long parsed = strtoll(text, &end, 10);
    if (errno == ERANGE || end == text || *end != '\0' || parsed < minimum || parsed > maximum) {
        return -1;
    }
    *value = parsed;
    return 0;
}

int main(int argc, char **argv) {
    long long timeout_ms = 10000;
    long long memory_bytes = 512LL * 1024 * 1024;
    long long stack_bytes = 8LL * 1024 * 1024;
    int pids_limit = 64;
    long long max_output = 1 << 20;
    const char *alloc_out = NULL;
    const char *alloc_xtree = NULL;
    const char *stdin_path = NULL;
    const char *emit_file = NULL;
    const char *working_directory = NULL;
    int collect_hardware = 0;
    int i = 1;
    long long parsed_limit = 0;

    for (; i < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        } else if (strcmp(argv[i], "--timeout-ms") == 0 && i + 1 < argc) {
            if (parse_integer(argv[++i], 1, INT_MAX, &parsed_limit) != 0) {
                fprintf(stderr, "ccx-runner: invalid --timeout-ms value\n");
                return EXIT_MISUSE;
            }
            timeout_ms = parsed_limit;
        } else if (strcmp(argv[i], "--memory-bytes") == 0 && i + 1 < argc) {
            if (parse_integer(argv[++i], 1, LLONG_MAX, &parsed_limit) != 0) {
                fprintf(stderr, "ccx-runner: invalid --memory-bytes value\n");
                return EXIT_MISUSE;
            }
            memory_bytes = parsed_limit;
        } else if (strcmp(argv[i], "--stack-bytes") == 0 && i + 1 < argc) {
            if (parse_integer(argv[++i], 1, LLONG_MAX, &parsed_limit) != 0) {
                fprintf(stderr, "ccx-runner: invalid --stack-bytes value\n");
                return EXIT_MISUSE;
            }
            stack_bytes = parsed_limit;
        } else if (strcmp(argv[i], "--pids-limit") == 0 && i + 1 < argc) {
            if (parse_integer(argv[++i], 1, INT_MAX, &parsed_limit) != 0) {
                fprintf(stderr, "ccx-runner: invalid --pids-limit value\n");
                return EXIT_MISUSE;
            }
            pids_limit = (int)parsed_limit;
        } else if (strcmp(argv[i], "--max-output-bytes") == 0 && i + 1 < argc) {
            if (parse_integer(argv[++i], 2, LLONG_MAX / 2, &parsed_limit) != 0) {
                fprintf(stderr, "ccx-runner: invalid --max-output-bytes value\n");
                return EXIT_MISUSE;
            }
            max_output = parsed_limit;
        } else if (strcmp(argv[i], "--alloc-out") == 0 && i + 1 < argc) {
            alloc_out = argv[++i];
        } else if (strcmp(argv[i], "--alloc-xtree") == 0 && i + 1 < argc) {
            alloc_xtree = argv[++i];
        } else if (strcmp(argv[i], "--stdin") == 0 && i + 1 < argc) {
            stdin_path = argv[++i];
        } else if (strcmp(argv[i], "--emit-file") == 0 && i + 1 < argc) {
            emit_file = argv[++i];
        } else if (strcmp(argv[i], "--cwd") == 0 && i + 1 < argc) {
            working_directory = argv[++i];
        } else if (strcmp(argv[i], "--hardware") == 0) {
            collect_hardware = 1;
        } else {
            fprintf(stderr, "ccx-runner: unknown argument %s\n", argv[i]);
            return EXIT_MISUSE;
        }
    }

    if (i >= argc) {
        fprintf(stderr, "ccx-runner: no program given\n");
        return EXIT_MISUSE;
    }
    if (alloc_out && alloc_xtree) {
        fprintf(stderr, "ccx-runner: allocation output modes are mutually exclusive\n");
        return EXIT_MISUSE;
    }
    if (prctl(PR_SET_CHILD_SUBREAPER, 1) != 0) {
        perror("ccx-runner: child subreaper");
        return EXIT_MISUSE;
    }

    /* Half the cap for each stream, so one noisy stream cannot starve the
       other's evidence. */
    struct capture out_cap, err_cap;
    if (capture_init(&out_cap) != 0 || capture_init(&err_cap) != 0) {
        fprintf(stderr, "ccx-runner: out of memory\n");
        return EXIT_MISUSE;
    }

    int out_pipe[2], err_pipe[2];
    if (pipe(out_pipe) != 0 || pipe(err_pipe) != 0) {
        perror("ccx-runner: pipe");
        return EXIT_MISUSE;
    }

    int gate_pipe[2] = {-1, -1};
    if (collect_hardware && pipe(gate_pipe) != 0) {
        collect_hardware = 0;
    }

    long long start = now_ns();

    pid_t pid = fork();
    if (pid < 0) {
        perror("ccx-runner: fork");
        return EXIT_MISUSE;
    }

    if (pid == 0) {
        /* Child: its own session, so a fork bomb is killable as a unit and
           cannot signal the runner. */
        setsid();
        close(out_pipe[0]);
        close(err_pipe[0]);
        if (gate_pipe[1] >= 0) {
            close(gate_pipe[1]);
            char gate = 0;
            ssize_t gate_status;
            do {
                gate_status = read(gate_pipe[0], &gate, 1);
            } while (gate_status < 0 && errno == EINTR);
            close(gate_pipe[0]);
            if (gate_status != 1) {
                _exit(EXIT_NOT_EXECUTABLE);
            }
        }
        dup2(out_pipe[1], STDOUT_FILENO);
        dup2(err_pipe[1], STDERR_FILENO);
        close(out_pipe[1]);
        close(err_pipe[1]);

        if (working_directory && chdir(working_directory) != 0) {
            perror("ccx-runner: chdir");
            _exit(EXIT_NOT_EXECUTABLE);
        }

        /* Tell the allocation shim where to write its accounting, if one is
           linked in. Harmless when it is not. */
        if (alloc_out) {
            setenv("CCX_ALLOC_OUT", alloc_out, 1);
        }

        if (apply_limits(memory_bytes, stack_bytes, pids_limit) != 0) {
            _exit(EXIT_MISUSE);
        }

        /* stdin is the test input when one was given, otherwise /dev/null: a
           solution must not block on input that will never come, and must not
           read the harness' own stdin. */
        int in_fd;
        if (stdin_path) {
            in_fd = open(stdin_path, O_RDONLY);
            if (in_fd < 0) {
                in_fd = open("/dev/null", O_RDONLY);
            }
        } else {
            in_fd = open("/dev/null", O_RDONLY);
        }
        if (in_fd >= 0) {
            dup2(in_fd, STDIN_FILENO);
            if (in_fd != STDIN_FILENO) {
                close(in_fd);
            }
        }

        /* The program is resolved against a fixed PATH set here, not against
           whatever the environment happens to hold: `g++` must mean the pinned
           compiler, and a submission must not be able to influence which binary
           the engine runs. execvp is used so a bare name (a compiler) and an
           absolute path (a compiled solution) both work. */
        setenv("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", 1);
        execvp(argv[i], &argv[i]);
        _exit(EXIT_NOT_EXECUTABLE);
    }

    /* Parent: drain both pipes with a deadline, capping total output. */
    close(out_pipe[1]);
    close(err_pipe[1]);
    struct hardware_fds hardware_fds = {-1, -1, -1, -1};
    if (collect_hardware && gate_pipe[0] >= 0) {
        close(gate_pipe[0]);
        hardware_fds.instructions = open_hardware_counter(pid, PERF_COUNT_HW_INSTRUCTIONS);
        hardware_fds.cache_misses = open_hardware_counter(pid, PERF_COUNT_HW_CACHE_MISSES);
        hardware_fds.branch_misses = open_hardware_counter(pid, PERF_COUNT_HW_BRANCH_MISSES);
        hardware_fds.cpu_cycles = open_hardware_counter(pid, PERF_COUNT_HW_CPU_CYCLES);
        char gate = 1;
        ssize_t released;
        do {
            released = write(gate_pipe[1], &gate, 1);
        } while (released < 0 && errno == EINTR);
        close(gate_pipe[1]);
        if (released == 1) {
            start = now_ns();
        }
    }
    fcntl(out_pipe[0], F_SETFL, O_NONBLOCK);
    fcntl(err_pipe[0], F_SETFL, O_NONBLOCK);

    long long out_bytes = 0, err_bytes = 0;
    int output_truncated = 0;
    int timed_out = 0;
    int open_fds = 2;
    int child_done = 0;
    int status = 0;
    struct rusage ru;
    memset(&ru, 0, sizeof ru);
    int process_fd = -1;
#ifdef SYS_pidfd_open
    process_fd = (int)syscall(SYS_pidfd_open, pid, 0);
#endif
    long long deadline = start + timeout_ms * 1000000LL;

    char buf[65536];
    while (open_fds > 0 || !child_done) {
        if (!child_done) {
            pid_t waited = wait4(pid, &status, WNOHANG, &ru);
            if (waited == pid) {
                child_done = 1;
                if (process_fd >= 0) {
                    close(process_fd);
                    process_fd = -1;
                }
            } else if (waited < 0 && errno != EINTR) {
                child_done = 1;
                if (process_fd >= 0) {
                    close(process_fd);
                    process_fd = -1;
                }
            }
        }
        if (open_fds == 0 && child_done) {
            break;
        }
        long long remaining = deadline - now_ns();
        if (remaining <= 0) {
            timed_out = 1;
            break;
        }
        struct pollfd fds[3];
        int n = 0;
        if (out_pipe[0] >= 0) { fds[n].fd = out_pipe[0]; fds[n].events = POLLIN; n++; }
        if (err_pipe[0] >= 0) { fds[n].fd = err_pipe[0]; fds[n].events = POLLIN; n++; }
        if (process_fd >= 0) { fds[n].fd = process_fd; fds[n].events = POLLIN; n++; }
        int wait_ms = (int)(remaining / 1000000LL);
        if (wait_ms <= 0) { wait_ms = 1; }
        if (process_fd < 0 && n == 0 && wait_ms > 10) { wait_ms = 10; }
        int pr = poll(fds, n, wait_ms);
        if (pr < 0) {
            if (errno == EINTR) { continue; }
            break;
        }

        for (int k = 0; k < n; k++) {
            if (fds[k].revents & (POLLIN | POLLHUP | POLLERR)) {
                ssize_t got;
                while ((got = read(fds[k].fd, buf, sizeof buf)) > 0) {
                    int is_out = (fds[k].fd == out_pipe[0]);
                    size_t captured_before = out_cap.len + err_cap.len;
                    size_t remaining = captured_before < (size_t)max_output
                        ? (size_t)max_output - captured_before
                        : 0;
                    if (is_out) {
                        out_bytes += got;
                        capture_append(&out_cap, buf, (size_t)got, remaining);
                    } else {
                        err_bytes += got;
                        capture_append(&err_cap, buf, (size_t)got, remaining);
                    }
                    if (out_cap.truncated || err_cap.truncated || out_bytes + err_bytes > max_output) {
                        output_truncated = 1;
                        break;
                    }
                }
                if (got == 0) {
                    close(fds[k].fd);
                    if (fds[k].fd == out_pipe[0]) { out_pipe[0] = -1; } else { err_pipe[0] = -1; }
                    open_fds--;
                }
            }
            if (output_truncated) {
                break;
            }
        }

        if (output_truncated || out_cap.truncated || err_cap.truncated || out_bytes + err_bytes > max_output) {
            output_truncated = 1;
            break;
        }
    }

    if (timed_out || output_truncated) {
        /* Kill the whole process group: a fork bomb has many members. */
        kill(-pid, SIGKILL);
        kill(pid, SIGKILL);
    } else {
        kill(-pid, SIGKILL);
    }
    if (process_fd >= 0) {
        close(process_fd);
    }

    if (!child_done) {
        while (wait4(pid, &status, 0, &ru) < 0) {
            if (errno != EINTR) { break; }
        }
    }
    int descendants = terminate_descendants();

    int emit_fd = -1;
    off_t emit_size = 0;
    int emit_file_too_large = 0;
    if (emit_file) {
        emit_fd = open(emit_file, O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK);
        if (emit_fd >= 0) {
            struct stat emit_stat;
            if (fstat(emit_fd, &emit_stat) == 0 && S_ISREG(emit_stat.st_mode) && emit_stat.st_size >= 0) {
                if (emit_stat.st_size > MAX_EMIT_BYTES) {
                    emit_file_too_large = 1;
                } else {
                    emit_size = emit_stat.st_size;
                }
            } else {
                close(emit_fd);
                emit_fd = -1;
            }
        }
    }

    long long instructions_hw = close_hardware_counter(hardware_fds.instructions);
    long long cache_misses_hw = close_hardware_counter(hardware_fds.cache_misses);
    long long branch_misses_hw = close_hardware_counter(hardware_fds.branch_misses);
    long long cpu_cycles_hw = close_hardware_counter(hardware_fds.cpu_cycles);

    long long wall_ns = now_ns() - start;

    int exit_status = 0;
    int signal_num = 0;
    if (WIFEXITED(status)) {
        exit_status = WEXITSTATUS(status);
    } else if (WIFSIGNALED(status)) {
        signal_num = WTERMSIG(status);
        /* A signaled child must never look like a clean exit. Shell
           convention: 128 + signal. The JSON also carries the raw signal, so
           the engine can tell a stack overflow (11) from an OOM kill (9). */
        exit_status = 128 + signal_num;
    }
    if (descendants < 0 || (descendants > 0 && exit_status == 0 && signal_num == 0)) {
        exit_status = EXIT_MISUSE;
    }
    int artifact_too_large = emit_file_too_large && exit_status == 0;
    if (artifact_too_large) {
        exit_status = EXIT_OUTPUT_TRUNCATED;
    }

    /* ru_maxrss is in kilobytes on Linux. */
    long long peak_rss_bytes = (long long)ru.ru_maxrss * 1024LL;
    long long user_cpu_ns = timeval_ns(ru.ru_utime);
    long long system_cpu_ns = timeval_ns(ru.ru_stime);
    char instructions_hw_json[32];
    char cache_misses_hw_json[32];
    char branch_misses_hw_json[32];
    char cpu_cycles_hw_json[32];
    snprintf(instructions_hw_json, sizeof instructions_hw_json, instructions_hw >= 0 ? "%lld" : "null", instructions_hw);
    snprintf(cache_misses_hw_json, sizeof cache_misses_hw_json, cache_misses_hw >= 0 ? "%lld" : "null", cache_misses_hw);
    snprintf(branch_misses_hw_json, sizeof branch_misses_hw_json, branch_misses_hw >= 0 ? "%lld" : "null", branch_misses_hw);
    snprintf(cpu_cycles_hw_json, sizeof cpu_cycles_hw_json, cpu_cycles_hw >= 0 ? "%lld" : "null", cpu_cycles_hw);

    /* Emit the framed report on the runner's own stdout. This fd is untouched
       in the child, so a submission cannot forge or corrupt the framing. */
    FILE *report = stdout;
    fprintf(report, "CCX-OUT %zu\n", out_cap.len);
    fwrite(out_cap.data, 1, out_cap.len, report);
    fprintf(report, "CCX-ERR %zu\n", err_cap.len);
    fwrite(err_cap.data, 1, err_cap.len, report);

    /* If an allocation shim was linked in, its accounting is a separate small
       JSON object. It is framed the same way: length-prefixed, so a submission
       cannot splice a value into it. An absent file is reported as 0 bytes. */
    char alloc_buf[512];
    size_t alloc_len = 0;
    if (alloc_xtree) {
        alloc_len = read_alloc_xtree(alloc_xtree, pid, alloc_buf, sizeof alloc_buf);
    } else if (alloc_out) {
        FILE *af = fopen(alloc_out, "r");
        if (af) {
            alloc_len = fread(alloc_buf, 1, sizeof alloc_buf, af);
            fclose(af);
        }
    }
    fprintf(report, "CCX-ALLOC %zu\n", alloc_len);
    fwrite(alloc_buf, 1, alloc_len, report);

    /* An optional file the caller asked to be carried out of the sandbox, for
       example a compiled binary. It is framed exactly like the other sections:
       length-prefixed, so its contents can never be mistaken for framing. The
       section is always present, as 0 bytes when no file was requested, so the
       reader's order is fixed. */
    fprintf(report, "CCX-EMIT ");
    if (emit_fd >= 0 && !emit_file_too_large) {
        fprintf(report, "%lld\n", (long long)emit_size);
        char buf[65536];
        off_t offset = 0;
        while (offset < emit_size) {
            size_t remaining = (size_t)(emit_size - offset);
            size_t amount = remaining < sizeof buf ? remaining : sizeof buf;
            ssize_t got = pread(emit_fd, buf, amount, offset);
            if (got < 0 && errno == EINTR) {
                continue;
            }
            if (got <= 0) {
                break;
            }
            fwrite(buf, 1, (size_t)got, report);
            offset += got;
        }
    } else {
        fprintf(report, "0\n");
    }
    if (emit_fd >= 0) {
        close(emit_fd);
    }

    fprintf(report,
            "CCX-VERDICT {\"exit_status\":%d,\"signal\":%d,\"timed_out\":%s,"
            "\"output_truncated\":%s,\"artifact_too_large\":%s,\"peak_rss_bytes\":%lld,\"wall_ns\":%lld,"
            "\"stdout_bytes\":%lld,\"stderr_bytes\":%lld,"
            "\"user_cpu_ns\":%lld,\"system_cpu_ns\":%lld,"
            "\"minor_page_faults\":%ld,\"major_page_faults\":%ld,"
            "\"voluntary_context_switches\":%ld,\"involuntary_context_switches\":%ld,"
            "\"block_input_ops\":%ld,\"block_output_ops\":%ld,"
            "\"instructions_hw\":%s,\"cache_misses_hw\":%s,\"branch_misses_hw\":%s,"
            "\"cpu_cycles_hw\":%s}\n",
            exit_status, signal_num,
            timed_out ? "true" : "false",
            output_truncated ? "true" : "false",
            artifact_too_large ? "true" : "false",
            peak_rss_bytes, wall_ns, out_bytes, err_bytes, user_cpu_ns, system_cpu_ns,
            ru.ru_minflt, ru.ru_majflt, ru.ru_nvcsw, ru.ru_nivcsw, ru.ru_inblock, ru.ru_oublock,
            instructions_hw_json, cache_misses_hw_json, branch_misses_hw_json, cpu_cycles_hw_json);
    fflush(report);

    free(out_cap.data);
    free(err_cap.data);

    if (timed_out) { return EXIT_TIMEOUT; }
    if (output_truncated) { return EXIT_OUTPUT_TRUNCATED; }
    return exit_status;
}
