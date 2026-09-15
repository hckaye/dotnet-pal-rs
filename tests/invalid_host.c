#include "dotnet_pal.h"
#include <assert.h>
#include <stdlib.h>
// A header-only old host must be rejected WITHOUT reading its absent function table.
static const dotnet_pal_header OLD = { 1, sizeof(dotnet_pal_header), 0 };
static const dotnet_pal_host_api EMPTY = {
    { DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_api), DOTNET_PAL_CAP_VM }, { 0 }
};
static int mode;
const dotnet_pal_host_api *dotnet_pal_host_v2(void) {
    if (mode == 0) return NULL;
    if (mode == 1) return (const dotnet_pal_host_api *)&OLD;
    return &EMPTY;
}
_Noreturn void dotnet_pal_host_abort(void) { abort(); }
int main(void) {
    for (mode = 0; mode < 3; ++mode) assert(dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION) == NULL);
    return 0;
}
