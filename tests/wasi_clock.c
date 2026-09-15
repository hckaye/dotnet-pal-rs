#include "dotnet_pal.h"
#define CHECK(x) do { if (!(x)) return __LINE__; } while (0)
uint32_t pal_clock_test(uint32_t injected_failure) {
    const dotnet_pal_api *p = dotnet_pal_get_api(2);
    CHECK(p && p->header.struct_size >= DOTNET_PAL_SERVICES_API_SIZE);
    CHECK(p->header.capabilities == (DOTNET_PAL_CAP_LINEAR | DOTNET_PAL_CAP_CLOCK));
    CHECK(p->services.monotonic_ns && p->services.read_stats);
    CHECK(!p->services.sleep_ns && !p->services.yield_thread);
    CHECK(!p->vm.reserve && !p->vm.commit);
    CHECK(p->services.monotonic_ns(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    _Alignas(uint64_t) unsigned char data[sizeof(uint64_t) + 1];
    CHECK(p->services.monotonic_ns((uint64_t *)(data + 1)) == DOTNET_PAL_INVALID_ARGUMENT);
    uint64_t before = UINT64_MAX, after = 0;
    uint32_t status = p->services.monotonic_ns(&before);
    if (injected_failure) {
        CHECK(status == DOTNET_PAL_OS_ERROR && before == 0);
    } else {
        CHECK(status == DOTNET_PAL_OK);
        CHECK(p->services.monotonic_ns(&after) == DOTNET_PAL_OK);
        CHECK(after >= before);
    }
    dotnet_pal_services_stats s;
    CHECK(p->services.read_stats(&s, sizeof s) == DOTNET_PAL_OK);
    CHECK(s.clock_ok == (injected_failure ? 0u : 2u));
    CHECK(s.rejected_or_failed == (injected_failure ? 3u : 2u));
    CHECK(s.sleep_ok == 0 && s.yield_ok == 0);
    return 0;
}
