#include "thread_identity.h"
#include <atomic>
#include <cassert>
#include <cstdio>
#include <thread>
static std::atomic<uintptr_t> next{1};
static std::atomic<unsigned> calls{0};
static uintptr_t current() {
    static thread_local uintptr_t id = next.fetch_add(1);
    ++calls;
    return id;
}
using Identity = dotnet_pal::ThreadIdentity<current>;
int main() {
    Identity main_id;
    assert(!main_id.IsCurrentThread() && calls.load()==0);
    main_id.Clear();
    assert(!main_id.IsCurrentThread() && calls.load()==0);
    main_id.SetToCurrentThread();
    assert(main_id.IsCurrentThread());
    Identity worker_id;
    std::thread worker([&] {
        assert(!main_id.IsCurrentThread());
        worker_id.SetToCurrentThread();
        assert(worker_id.IsCurrentThread());
        worker_id.Clear();
        unsigned before = calls.load();
        assert(!worker_id.IsCurrentThread());
        assert(before == calls.load());
    });
    worker.join();
    assert(main_id.IsCurrentThread());
    assert(!worker_id.IsCurrentThread());
    main_id.Clear();
    unsigned before = calls.load();
    assert(!main_id.IsCurrentThread());
    assert(before == calls.load());
    main_id.SetToCurrentThread();
    assert(main_id.IsCurrentThread());
    std::puts("GC THREAD IDENTITY PASS validity, ownership, cross-thread mismatch and clear");
}
