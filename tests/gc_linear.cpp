#include "gc_linear_adapter.h"
#include "gc_vm_adapter.h"
#include <cassert>
#include <cstring>
#include <thread>
#include <vector>

static void model_test(size_t page) {
    using namespace dotnet_pal_gc_linear;
    struct Live { void *address; size_t size; unsigned char value; };
    Live live[64]{};
    uint32_t seed = 0x781fe234;
    auto next = [&seed]() { seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5; return seed; };
    for (int step = 0; step < 4096; ++step) {
        auto &slot = live[next() % 64];
        if (!slot.address) {
            slot.size = page * (1 + next() % 8);
            slot.address = reserve(slot.size, page << (next() % 3), 0, 0);
            assert(slot.address);
            for (size_t i = 0; i < slot.size; ++i) assert(static_cast<unsigned char *>(slot.address)[i] == 0);
            slot.value = static_cast<unsigned char>(next());
            std::memset(slot.address, slot.value, slot.size);
        } else {
            assert(commit(slot.address, slot.size, 0));
            for (size_t i = 0; i < slot.size; ++i) assert(static_cast<unsigned char *>(slot.address)[i] == slot.value);
            if (next() & 1) {
                assert(decommit(slot.address, slot.size));
                slot.value = 0;
            } else {
                assert(release(slot.address, slot.size)); slot = {};
            }
        }
    }
    for (auto &slot : live) if (slot.address) assert(release(slot.address, slot.size));
    assert(snapshot().owned_bytes == 0);
}
int main() {
    using namespace dotnet_pal_gc_linear;
    assert(api() && !dotnet_pal_gc::api());
    assert((api()->header.capabilities & DOTNET_PAL_CAP_VM) == 0);
    const size_t page = api()->linear.granularity();
    void *p = reserve(page * 3, page * 16, 0, UINT16_MAX);
    assert(p && reinterpret_cast<uintptr_t>(p) % (page * 16) == 0);
    assert(commit(p, page * 3, 0));
    std::memset(p, 0x67, page * 3);
    void *unaligned = static_cast<unsigned char *>(p) + 1;
    assert(!commit(unaligned, page, 0));
    assert(!decommit(unaligned, page));
    assert(!reset(unaligned, page, false));
    assert(!release(unaligned, page));
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
    assert(!reserve(page, page, 0, 0));
    for (auto r : held) assert(release(r, page));
    model_test(page);
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
