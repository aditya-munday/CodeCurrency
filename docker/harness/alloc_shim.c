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
#include <errno.h>
#include <stdatomic.h>
#include <stdint.h>
#include <stddef.h>

extern void *__real_malloc(size_t);
extern void *__real_calloc(size_t, size_t);
extern void *__real_realloc(void *, size_t);
extern void __real_free(void *);

typedef union AllocationHeader {
    max_align_t alignment;
    struct {
        void *base;
        size_t size;
    } value;
} AllocationHeader;

static _Atomic unsigned long long g_allocations = 0;
static _Atomic unsigned long long g_allocated_bytes = 0;
static _Atomic unsigned long long g_frees = 0;
static _Atomic unsigned long long g_peak_live_bytes = 0;
static _Atomic unsigned long long g_live_bytes = 0;
static _Atomic int g_reported = 0;
static _Thread_local int g_suspended = 0;

static void account_alloc(size_t bytes) {
    if (g_suspended) {
        return;
    }
    atomic_fetch_add_explicit(&g_allocations, 1, memory_order_relaxed);
    atomic_fetch_add_explicit(&g_allocated_bytes, bytes, memory_order_relaxed);
    unsigned long long live = atomic_fetch_add_explicit(&g_live_bytes, bytes, memory_order_relaxed) + bytes;
    unsigned long long peak = atomic_load_explicit(&g_peak_live_bytes, memory_order_relaxed);
    while (live > peak && !atomic_compare_exchange_weak_explicit(
               &g_peak_live_bytes, &peak, live, memory_order_relaxed, memory_order_relaxed)) {
    }
}

static void account_free(size_t bytes) {
    if (g_suspended) {
        return;
    }
    atomic_fetch_add_explicit(&g_frees, 1, memory_order_relaxed);
    unsigned long long live = atomic_load_explicit(&g_live_bytes, memory_order_relaxed);
    while (!atomic_compare_exchange_weak_explicit(
        &g_live_bytes,
        &live,
        bytes <= live ? live - bytes : 0,
        memory_order_relaxed,
        memory_order_relaxed
    )) {
    }
}

/*
 * The size of an allocation is tracked in a header placed before the returned
 * pointer, so free() can account for exactly what malloc() reserved. This keeps
 * `live_bytes` honest across realloc and mismatched allocators.
 */
static AllocationHeader *header_of(void *ptr) {
    return (AllocationHeader *)ptr - 1;
}

void __wrap_free(void *ptr);

static void *allocate_aligned(size_t size, size_t alignment, int clear) {
    if (alignment < sizeof(void *) || (alignment & (alignment - 1)) != 0) {
        errno = EINVAL;
        return NULL;
    }
    if (size > SIZE_MAX - sizeof(AllocationHeader) - alignment + 1) {
        errno = ENOMEM;
        return NULL;
    }
    size_t total = size + sizeof(AllocationHeader) + alignment - 1;
    void *base = __real_malloc(total);
    if (!base) {
        return NULL;
    }
    uintptr_t start = (uintptr_t)base + sizeof(AllocationHeader);
    uintptr_t aligned = (start + alignment - 1) & ~(uintptr_t)(alignment - 1);
    AllocationHeader *header = (AllocationHeader *)aligned - 1;
    header->value.base = base;
    header->value.size = size;
    void *result = (void *)aligned;
    if (clear && size > 0) {
        memset(result, 0, size);
    }
    account_alloc(size);
    return result;
}

void *__wrap_malloc(size_t size) {
    return allocate_aligned(size, _Alignof(max_align_t), 0);
}

void *__wrap_calloc(size_t n, size_t size) {
    if (size != 0 && n > SIZE_MAX / size) {
        errno = ENOMEM;
        return NULL;
    }
    return allocate_aligned(n * size, _Alignof(max_align_t), 1);
}

void *__wrap_realloc(void *ptr, size_t size) {
    if (!ptr) {
        return __wrap_malloc(size);
    }
    if (size == 0) {
        __wrap_free(ptr);
        return NULL;
    }
    AllocationHeader *header = header_of(ptr);
    size_t old_size = header->value.size;
    void *next = allocate_aligned(size, _Alignof(max_align_t), 0);
    if (!next) {
        return NULL;
    }
    memcpy(next, ptr, old_size < size ? old_size : size);
    __wrap_free(ptr);
    return next;
}

void __wrap_free(void *ptr) {
    if (!ptr) {
        return;
    }
    AllocationHeader *header = header_of(ptr);
    size_t size = header->value.size;
    void *base = header->value.base;
    account_free(size);
    __real_free(base);
}

int __wrap_posix_memalign(void **memptr, size_t alignment, size_t size) {
    if (alignment < sizeof(void *) || (alignment & (alignment - 1)) != 0) {
        return EINVAL;
    }
    void *result = allocate_aligned(size, alignment, 0);
    if (!result) {
        return errno == EINVAL ? EINVAL : ENOMEM;
    }
    *memptr = result;
    return 0;
}

void *__wrap_aligned_alloc(size_t alignment, size_t size) {
    if (alignment == 0 || size % alignment != 0) {
        errno = EINVAL;
        return NULL;
    }
    return allocate_aligned(size, alignment, 0);
}

void *__wrap_memalign(size_t alignment, size_t size) {
    return allocate_aligned(size, alignment, 0);
}

__attribute__((destructor)) static void report(void) {
    if (atomic_exchange_explicit(&g_reported, 1, memory_order_relaxed)) {
        return;
    }
    g_suspended = 1;
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
            atomic_load_explicit(&g_allocations, memory_order_relaxed),
            atomic_load_explicit(&g_allocated_bytes, memory_order_relaxed),
            atomic_load_explicit(&g_frees, memory_order_relaxed),
            atomic_load_explicit(&g_peak_live_bytes, memory_order_relaxed));
    fclose(f);
}
