#include "unwind_lock.h"
#include <cassert>
#include <cstdio>
#include <pthread.h>
static DotnetPalUnwindLock lock;
static unsigned count;
static void *worker(void*) {
    for (unsigned i = 0; i < 10000; ++i) {
        assert(lock.lock_shared());
        assert(lock.lock_shared()); // recursive shared acquisition
        ++count;
        assert(lock.unlock_shared()); assert(lock.unlock_shared());
        assert(lock.lock()); ++count; assert(lock.unlock());
    }
    return nullptr;
}
int main() {
    assert(!lock.unlock());
    pthread_t threads[8];
    for (int i = 0; i < 8; ++i) assert(pthread_create(&threads[i], nullptr, worker, nullptr) == 0);
    for (auto t : threads) assert(pthread_join(t, nullptr) == 0);
    assert(count == 160000);
    assert(lock.lock()); assert(!lock.destroy()); assert(lock.unlock());
    assert(lock.destroy()); assert(lock.destroy());
    puts("UNWIND LOCK PASS neutral capability, concurrent initialization, recursive read, exclusive write, busy destruction");
}
