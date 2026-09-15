/* Test-only reverse-P/Invoke probe. Not linked into the PAL library. */
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
typedef int (*managed_entry)(intptr_t);
struct call { managed_entry entry; intptr_t argument; int result; };
static void *invoke(void *p) {
    struct call *c = p;
    c->result = c->entry(c->argument);
    return 0;
}
int pal_qualification_foreign_thread(managed_entry entry, intptr_t argument) {
    if (!entry) return -1;
    struct call c = {entry, argument, -1};
    pthread_t thread;
    int error = pthread_create(&thread, 0, invoke, &c);
    if (error) return error;
    error = pthread_join(thread, 0);
    if (error) abort(); /* argument storage must not expire while the worker lives */
    return c.result;
}
#ifndef PAL_FAULT_PROVIDER
uint32_t pal_qualification_fault_control(uint32_t count) { (void)count; return 1; }
uint64_t pal_qualification_fault_hits(void) { return 0; }
#endif
