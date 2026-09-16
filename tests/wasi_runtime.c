#include "dotnet_pal.h"
#define REQUIRE(x) do { if (!(x)) return __LINE__; } while(0)
int pal_wasi_runtime_test(int mode) {
    const dotnet_pal_api *a = dotnet_pal_get_api(2);
    REQUIRE(a && a->header.struct_size >= DOTNET_PAL_RUNTIME_API_SIZE);
    REQUIRE((a->header.capabilities & (DOTNET_PAL_CAP_ENVIRONMENT|DOTNET_PAL_CAP_REALTIME|DOTNET_PAL_CAP_ENTROPY)) ==
        (DOTNET_PAL_CAP_ENVIRONMENT|DOTNET_PAL_CAP_REALTIME|DOTNET_PAL_CAP_ENTROPY));
    REQUIRE(!(a->header.capabilities & (DOTNET_PAL_CAP_VM|DOTNET_PAL_CAP_IDENTITY|DOTNET_PAL_CAP_NATIVE_MEMORY|DOTNET_PAL_CAP_MODULES)));
    REQUIRE(!a->runtime.mapping_allocate && !a->runtime.process_id && !a->runtime.module_open);
    const dotnet_pal_runtime_ops *r = &a->runtime;
    uint8_t value[32]; for (int i=0;i<32;++i) value[i]=42;
    size_t needed=99; uint64_t ns=99;
    if (mode == 1 || mode == 4 || mode == 5 || mode == 6) {
        REQUIRE(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, value, sizeof value, &needed) ==
            (mode == 4 ? DOTNET_PAL_OUT_OF_MEMORY : DOTNET_PAL_OS_ERROR));
        REQUIRE(needed == 0 && value[0] == 0); return 0;
    }
    if (mode == 2) {
        REQUIRE(r->random_bytes(value, sizeof value) == DOTNET_PAL_OS_ERROR);
        for (int i=0;i<32;++i) REQUIRE(value[i] == 0);
        return 0;
    }
    if (mode == 3) {
        REQUIRE(r->realtime_ns(&ns) == DOTNET_PAL_OS_ERROR && ns == 0); return 0;
    }
    REQUIRE(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, NULL, 0, &needed) == DOTNET_PAL_BUFFER_TOO_SMALL && needed == 4);
    REQUIRE(r->environment_get((const uint8_t*)"PAL_RUNTIME_VALUE", 17, value, sizeof value, &needed) == 0);
    REQUIRE(needed == 4 && value[0]=='a' && value[1]=='b' && value[2]=='c' && value[3]==0);
    REQUIRE(r->environment_get((const uint8_t*)"PAL_RUNTIME_EMPTY", 17, value, sizeof value, &needed) == 0 && needed == 1 && value[0] == 0);
    REQUIRE(r->environment_get((const uint8_t*)"PAL_RUNTIME_MISSING", 19, value, sizeof value, &needed) == DOTNET_PAL_NOT_FOUND && needed == 0 && value[0] == 0);
    REQUIRE(r->realtime_ns(&ns) == 0 && ns > UINT64_C(946684800000000000));
    REQUIRE(r->realtime_ns(NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    REQUIRE(r->random_bytes(value, sizeof value) == 0);
    REQUIRE(r->random_bytes(NULL, 0) == 0);
    REQUIRE(r->random_bytes(NULL, 1) == DOTNET_PAL_INVALID_ARGUMENT);
    dotnet_pal_runtime_stats stats;
    REQUIRE(r->read_stats(&stats, sizeof stats) == 0 && stats.environment_ok == 2 && stats.entropy_ok == 2 && stats.realtime_ok == 1);
    return 0;
}
