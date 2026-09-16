#include "unwind_lock.h"
#include <cassert>
#include <cstdio>
#include <thread>
#include <vector>
static DotnetPalUnwindLock lock;
static unsigned count;
static void worker() {
    for (unsigned i = 0; i < 10000; ++i) {
        assert(lock.lock_shared());
        assert(lock.lock_shared()); // recursive shared acquisition
        ++count;
        assert(lock.unlock_shared()); assert(lock.unlock_shared());
        assert(lock.lock()); ++count; assert(lock.unlock());
    }
}
int main() {
    assert(!lock.unlock());
    std::vector<std::thread> threads;
    for (int i = 0; i < 8; ++i) threads.emplace_back(worker);
    for (auto &t : threads) t.join();
    assert(count == 160000);
    assert(lock.lock()); assert(!lock.destroy()); assert(lock.unlock());
    assert(lock.destroy()); assert(lock.destroy());
    puts("UNWIND LOCK PASS neutral capability, concurrent initialization, recursive read, exclusive write, busy destruction");
}
