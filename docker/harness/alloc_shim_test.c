#define _GNU_SOURCE
#include <assert.h>
#include <malloc.h>
#include <pthread.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>

static void *worker(void *arg) {
    size_t rounds = (size_t)arg;
    for (size_t i = 0; i < rounds; ++i) {
        void *p = malloc(37 + i % 19);
        assert(p != NULL);
        free(p);
    }
    return NULL;
}

int main(void) {
    void *p = malloc(33);
    assert(p != NULL);
    assert((uintptr_t)p % _Alignof(max_align_t) == 0);
    p = realloc(p, 257);
    assert(p != NULL);

    void *aligned = aligned_alloc(64, 256);
    assert(aligned != NULL);
    assert((uintptr_t)aligned % 64 == 0);

    void *posix = NULL;
    assert(posix_memalign(&posix, 256, 1024) == 0);
    assert((uintptr_t)posix % 256 == 0);

    void *zeroed = calloc(128, 2);
    assert(zeroed != NULL);
    for (size_t i = 0; i < 256; ++i) {
        assert(((unsigned char *)zeroed)[i] == 0);
    }

    free(p);
    free(aligned);
    free(posix);
    free(zeroed);

    pthread_t threads[8];
    for (size_t i = 0; i < 8; ++i) {
        assert(pthread_create(&threads[i], NULL, worker, (void *)1000) == 0);
    }
    for (size_t i = 0; i < 8; ++i) {
        assert(pthread_join(threads[i], NULL) == 0);
    }
    return 0;
}
