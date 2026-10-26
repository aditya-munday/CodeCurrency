/*
 * libccxalloc — allocation accounting shim.
 *
 * Linked into a submission with `-Wl,--wrap=malloc,--wrap=calloc,--wrap=realloc,
 * --wrap=free`. It counts allocations and bytes actually requested, then writes
 * a single JSON line at exit to the path in CCX_ALLOC_OUT.
 *
 * The shim must never change program behaviour or timing in a way that matters:
 * it only increments counters and forwards to the real allocator. It is a
 * *measured* metric (the program's own request stream), not a simulated one.
 *
 * If CCX_ALLOC_OUT is unset the shim still counts but writes nothing, so a
 * solution can be built with the shim in place and run without it.
 */

#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);
extern void __real_free(void *);

static unsigned long long g_allocations = 0;
static unsigned long long g_allocated_bytes = 0;
static unsigned long long g_frees = 0;
static unsigned long long g_peak_live_bytes = 0;
static unsigned long long g_live_bytes = 0;
static int g_reported = 0;

static void account_alloc(size_t bytes) {
    g_allocations++;
    g_allocated_bytes += bytes;
    g_live_bytes += bytes;
    if (g_live_bytes > g_peak_live_bytes) {
        g_peak_live_bytes = g_live_bytes;
    }
}

static void account_free(size_t bytes) {
    g_frees++;
    if (bytes <= g_live_bytes) {
        g_live_bytes -= bytes;
    } else {
        g_live_bytes = 0;
    }
}

/*
 * The size of an allocation is tracked in a header placed before the returned
 * pointer, so free() can account for exactly what malloc() reserved. This keeps
 * `live_bytes` honest across realloc and mismatched allocators.
 */
static size_t *header_of(void *ptr) {
    return (size_t *)ptr - 1;
}

void *__wrap_malloc(size_t size) {
    size_t *raw = (size_t *)__real_malloc(size + sizeof(size_t));
    if (!raw) {
        return NULL;
    }
    raw[0] = size;
    account_alloc(size);
    return raw + 1;
}

void *__wrap_calloc(size_t n, size_t size) {
    size_t total = n * size;
    size_t *raw = (size_t *)__real_calloc(1, total + sizeof(size_t));
    if (!raw) {
        return NULL;
    }
    raw[0] = total;
    account_alloc(total);
    return raw + 1;
}

void *__wrap_realloc(void *ptr, size_t size) {
    if (!ptr) {
        return __wrap_malloc(size);
    }
    size_t *raw = header_of(ptr);
    size_t old = raw[0];
    size_t *next = (size_t *)__real_realloc(raw, size + sizeof(size_t));
    if (!next) {
        return NULL;
    }
    next[0] = size;
    account_alloc(size);
    account_free(old);
    return next + 1;
}

void __wrap_free(void *ptr) {
    if (!ptr) {
        return;
    }
    size_t *raw = header_of(ptr);
    size_t size = raw[0];
    account_free(size);
    __real_free(raw);
}

__attribute__((destructor)) static void report(void) {
    if (g_reported) {
        return;
    }
    g_reported = 1;
    const char *path = getenv("CCX_ALLOC_OUT");
    if (!path || !*path) {
        return;
    }
    FILE *f = fopen(path, "w");
    if (!f) {
        return;
    }
    fprintf(f,
            "{\"allocations\":%llu,\"allocated_bytes\":%llu,\"frees\":%llu,"
            "\"peak_live_bytes\":%llu}\n",
            g_allocations, g_allocated_bytes, g_frees, g_peak_live_bytes);
    fclose(f);
}
