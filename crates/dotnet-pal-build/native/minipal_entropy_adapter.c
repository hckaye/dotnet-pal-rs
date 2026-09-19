/* Audited minipal ABI: 0 success, -1 failure, no exception crosses this boundary.
 * WASIp1 has random_get rather than a generally accessible /dev/urandom path.
 * Route runtime AND BCL entropy through the same negotiated PAL capability.
 * Both minipal definitions live here so a static archive cannot reintroduce the
 * original random.c object merely to satisfy its non-cryptographic entry point.
 */
#include "dotnet_pal.h"
#include <errno.h>
#include <stdlib.h>
int32_t minipal_get_cryptographically_secure_random_bytes(uint8_t *out,int32_t length) {
    if(length<0 || (length>0 && !out)){errno=EINVAL;return -1;}
    const dotnet_pal_api *a=dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if(!a || a->header.struct_size<DOTNET_PAL_RUNTIME_API_SIZE || !(a->header.capabilities&DOTNET_PAL_CAP_ENTROPY) || !a->runtime.random_bytes){
        errno=ENOSYS;return -1;
    }
    if(a->runtime.random_bytes(out,(size_t)length)!=DOTNET_PAL_OK){errno=EIO;return -1;}
    return 0;
}
void minipal_get_non_cryptographically_secure_random_bytes(uint8_t *out,int32_t length) {
    // Never replace a failed entropy source with fake random bytes or success.
    if(minipal_get_cryptographically_secure_random_bytes(out,length)!=0)abort();
}
