#include "gc_linear_adapter.h"
#include "gc_vm_adapter.h"
#include <cassert>
#include <cstring>
#include <thread>
#include <vector>
int main() {
    using namespace dotnet_pal_gc_linear;
    assert(api() && !dotnet_pal_gc::api());
    assert((api()->header.capabilities & DOTNET_PAL_CAP_VM) == 0);
    const size_t page = api()->linear.granularity();
    void *p = reserve(page * 3, page * 16, 0, UINT16_MAX);
    assert(p && reinterpret_cast<uintptr_t>(p) % (page * 16) == 0);
    assert(commit(p, page * 3, 0));
    std::memset(p, 0x67, page * 3);
    assert(commit(p, page * 3, 0));
    for (size_t i = 0; i < page * 3; ++i) assert(static_cast<unsigned char *>(p)[i] == 0x67);
    void *middle = static_cast<unsigned char *>(p) + page;
    assert(decommit(middle, page)); assert(commit(middle, page, 0));
    for (size_t i = 0; i < page; ++i) {
        assert(static_cast<unsigned char *>(p)[i] == 0x67);
        assert(static_cast<unsigned char *>(middle)[i] == 0);
        assert(static_cast<unsigned char *>(p)[2 * page + i] == 0x67);
    }
    assert(!release(middle, page)); assert(!commit(static_cast<unsigned char *>(p) + page * 3, page, 0));
    assert(!reserve(page, 0, 1, 0)); assert(!reserve(SIZE_MAX, 0, 0, 0));
    assert(release(p, page * 3)); assert(!release(p, page * 3));
    assert(!commit(p, page, 0));
    void *held[256]{};
    for (auto &r : held) { r = reserve(page, page, 0, 0); assert(r); }
    assert(!reserve(page, page, 0, 0)); // bounded ledger exhaustion, not an unchecked overwrite
    for (auto r : held) assert(release(r, page));
    std::vector<std::thread> workers;
    for (int t = 0; t < 4; ++t) workers.emplace_back([page] {
        for (int i = 0; i < 128; ++i) {
            void *q = reserve(page * 2, page, 0, 0); assert(q);
            assert(commit(q, page * 2, 0)); std::memset(q, 1, page * 2);
            assert(reset(q, page * 2, false)); assert(release(q, page * 2));
        }
    });
    for (auto &w : workers) w.join();
    assert(snapshot().owned_bytes == 0);
    dotnet_pal_linear_stats stats{};
    assert(api()->linear.read_stats(&stats, sizeof(stats)) == DOTNET_PAL_OK);
    assert(stats.allocate_ok == stats.release_ok && stats.allocate_ok > 256);
}
