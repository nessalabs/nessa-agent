#define _GNU_SOURCE
#include <pthread.h>
#include <dlfcn.h>
#include <stdatomic.h>
#include <errno.h>
#include <stdio.h>
#include <unistd.h>

static _Atomic unsigned calls;
int pthread_create(pthread_t *thread, const pthread_attr_t *attr,
                   void *(*start)(void *), void *arg) {
    unsigned n = atomic_fetch_add(&calls, 1) + 1;
    if (n == 2) {
        dprintf(STDERR_FILENO, "627 probe: pthread_create #%u -> EAGAIN (injected initial blocking-worker refusal)\n", n);
        return EAGAIN;
    }
    int (*real_create)(pthread_t *, const pthread_attr_t *, void *(*)(void *), void *) = dlsym(RTLD_NEXT, "pthread_create");
    int result = real_create(thread, attr, start, arg);
    dprintf(STDERR_FILENO, "627 probe: pthread_create #%u -> %d (delegated)\n", n, result);
    return result;
}
