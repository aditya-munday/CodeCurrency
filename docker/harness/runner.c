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
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/resource.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define EXIT_TIMEOUT 124
#define EXIT_OUTPUT_TRUNCATED 125
#define EXIT_NOT_EXECUTABLE 126
#define EXIT_MISUSE 2

static long long now_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (long long)ts.tv_sec * 1000000000LL + ts.tv_nsec;
}

/* A bounded byte buffer for captured child output. */
struct capture {
    char *data;
    size_t len;
    size_t cap;
    int truncated;
};

static int capture_init(struct capture *c, size_t cap) {
    c->data = malloc(cap + 1);
    if (!c->data) {
        return -1;
    }
    c->len = 0;
    c->cap = cap;
    c->truncated = 0;
    return 0;
}

static void capture_append(struct capture *c, const char *buf, size_t n) {
    if (c->len >= c->cap) {
        c->truncated = 1;
        return;
    }
    size_t room = c->cap - c->len;
    size_t take = n < room ? n : room;
    memcpy(c->data + c->len, buf, take);
    c->len += take;
    if (take < n) {
        c->truncated = 1;
    }
}

/* Set every limit that keeps a hostile submission from escaping. */
static void apply_limits(long long memory_bytes, long long stack_bytes, int pids_limit) {
    struct rlimit rl;

    rl.rlim_cur = rl.rlim_max = (rlim_t)memory_bytes;
    setrlimit(RLIMIT_AS, &rl);

    rl.rlim_cur = rl.rlim_max = (rlim_t)stack_bytes;
    setrlimit(RLIMIT_STACK, &rl);

    rl.rlim_cur = rl.rlim_max = (rlim_t)pids_limit;
    setrlimit(RLIMIT_NPROC, &rl);

    /* No core dumps: they would be written into the read-only rootfs and could
       leak memory contents. */
    rl.rlim_cur = rl.rlim_max = 0;
    setrlimit(RLIMIT_CORE, &rl);
}

int main(int argc, char **argv) {
    long long timeout_ms = 10000;
    long long memory_bytes = 512LL * 1024 * 1024;
    long long stack_bytes = 8LL * 1024 * 1024;
    int pids_limit = 64;
    long long max_output = 1 << 20;
    const char *alloc_out = NULL;
    const char *stdin_path = NULL;
    const char *emit_file = NULL;
    int i = 1;

    for (; i < argc; i++) {
        if (strcmp(argv[i], "--") == 0) {
            i++;
            break;
        } else if (strcmp(argv[i], "--timeout-ms") == 0 && i + 1 < argc) {
            timeout_ms = atoll(argv[++i]);
        } else if (strcmp(argv[i], "--memory-bytes") == 0 && i + 1 < argc) {
            memory_bytes = atoll(argv[++i]);
        } else if (strcmp(argv[i], "--stack-bytes") == 0 && i + 1 < argc) {
            stack_bytes = atoll(argv[++i]);
        } else if (strcmp(argv[i], "--pids-limit") == 0 && i + 1 < argc) {
            pids_limit = atoi(argv[++i]);
        } else if (strcmp(argv[i], "--max-output-bytes") == 0 && i + 1 < argc) {
            max_output = atoll(argv[++i]);
        } else if (strcmp(argv[i], "--alloc-out") == 0 && i + 1 < argc) {
            alloc_out = argv[++i];
        } else if (strcmp(argv[i], "--stdin") == 0 && i + 1 < argc) {
            stdin_path = argv[++i];
        } else if (strcmp(argv[i], "--emit-file") == 0 && i + 1 < argc) {
            emit_file = argv[++i];
        } else {
            fprintf(stderr, "ccx-runner: unknown argument %s\n", argv[i]);
            return EXIT_MISUSE;
        }
    }

    if (i >= argc) {
        fprintf(stderr, "ccx-runner: no program given\n");
        return EXIT_MISUSE;
    }

    /* Half the cap for each stream, so one noisy stream cannot starve the
       other's evidence. */
    size_t per_stream = (size_t)(max_output / 2);
    struct capture out_cap, err_cap;
    if (capture_init(&out_cap, per_stream) != 0 || capture_init(&err_cap, per_stream) != 0) {
        fprintf(stderr, "ccx-runner: out of memory\n");
        return EXIT_MISUSE;
    }

    int out_pipe[2], err_pipe[2];
    if (pipe(out_pipe) != 0 || pipe(err_pipe) != 0) {
        perror("ccx-runner: pipe");
        return EXIT_MISUSE;
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
        dup2(out_pipe[1], STDOUT_FILENO);
        dup2(err_pipe[1], STDERR_FILENO);
        close(out_pipe[1]);
        close(err_pipe[1]);

        /* Tell the allocation shim where to write its accounting, if one is
           linked in. Harmless when it is not. */
        if (alloc_out) {
            setenv("CCX_ALLOC_OUT", alloc_out, 1);
        }

        apply_limits(memory_bytes, stack_bytes, pids_limit);

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
    fcntl(out_pipe[0], F_SETFL, O_NONBLOCK);
    fcntl(err_pipe[0], F_SETFL, O_NONBLOCK);

    long long out_bytes = 0, err_bytes = 0;
    int output_truncated = 0;
    int timed_out = 0;
    int open_fds = 2;
    long long deadline = start + timeout_ms * 1000000LL;

    char buf[65536];
    while (open_fds > 0) {
        long long remaining = deadline - now_ns();
        if (remaining <= 0) {
            timed_out = 1;
            break;
        }
        struct pollfd fds[2];
        int n = 0;
        if (out_pipe[0] >= 0) { fds[n].fd = out_pipe[0]; fds[n].events = POLLIN; n++; }
        if (err_pipe[0] >= 0) { fds[n].fd = err_pipe[0]; fds[n].events = POLLIN; n++; }
        if (n == 0) {
            break;
        }

        int wait_ms = (int)(remaining / 1000000LL);
        if (wait_ms <= 0) { wait_ms = 1; }
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
                    if (is_out) {
                        out_bytes += got;
                        capture_append(&out_cap, buf, (size_t)got);
                    } else {
                        err_bytes += got;
                        capture_append(&err_cap, buf, (size_t)got);
                    }
                }
                if (got == 0) {
                    close(fds[k].fd);
                    if (fds[k].fd == out_pipe[0]) { out_pipe[0] = -1; } else { err_pipe[0] = -1; }
                    open_fds--;
                }
            }
        }

        if (out_bytes + err_bytes > max_output) {
            output_truncated = 1;
            break;
        }
    }

    if (timed_out || output_truncated) {
        /* Kill the whole process group: a fork bomb has many members. */
        kill(-pid, SIGKILL);
        kill(pid, SIGKILL);
    }

    int status = 0;
    struct rusage ru;
    memset(&ru, 0, sizeof ru);
    while (wait4(pid, &status, 0, &ru) < 0) {
        if (errno != EINTR) { break; }
    }

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

    /* ru_maxrss is in kilobytes on Linux. */
    long long peak_rss_bytes = (long long)ru.ru_maxrss * 1024LL;

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
    if (alloc_out) {
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
    if (emit_file) {
        FILE *ef = fopen(emit_file, "rb");
        if (ef) {
            fseek(ef, 0, SEEK_END);
            long n = ftell(ef);
            fseek(ef, 0, SEEK_SET);
            if (n < 0) {
                n = 0;
            }
            fprintf(report, "%ld\n", n);
            char buf[65536];
            size_t got;
            while ((got = fread(buf, 1, sizeof buf, ef)) > 0) {
                fwrite(buf, 1, got, report);
            }
            fclose(ef);
        } else {
            fprintf(report, "0\n");
        }
    } else {
        fprintf(report, "0\n");
    }

    fprintf(report,
            "CCX-VERDICT {\"exit_status\":%d,\"signal\":%d,\"timed_out\":%s,"
            "\"output_truncated\":%s,\"peak_rss_bytes\":%lld,\"wall_ns\":%lld,"
            "\"stdout_bytes\":%lld,\"stderr_bytes\":%lld}\n",
            exit_status, signal_num,
            timed_out ? "true" : "false",
            output_truncated ? "true" : "false",
            peak_rss_bytes, wall_ns, out_bytes, err_bytes);
    fflush(report);

    free(out_cap.data);
    free(err_cap.data);

    if (timed_out) { return EXIT_TIMEOUT; }
    if (output_truncated) { return EXIT_OUTPUT_TRUNCATED; }
    return exit_status;
}
