#include "dotnet_pal.h"
#include <assert.h>
#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <stdio.h>
extern int32_t minipal_get_cryptographically_secure_random_bytes(uint8_t*,int32_t);
static int error,absent;
static uint32_t fill(uint8_t *out,size_t length){if(error)return DOTNET_PAL_OS_ERROR;memset(out,37,length);return 0;}
static dotnet_pal_api api;
const dotnet_pal_api *dotnet_pal_get_api(uint32_t version){assert(version==2);return absent?NULL:&api;}
int main(void){
    api.header=(dotnet_pal_header){2,sizeof api,DOTNET_PAL_CAP_ENTROPY};api.runtime.random_bytes=fill;
    uint8_t bytes[8]={0};
    assert(minipal_get_cryptographically_secure_random_bytes(bytes,8)==0 && bytes[0]==37);
    assert(minipal_get_cryptographically_secure_random_bytes(NULL,1)==-1 && errno==EINVAL);
    assert(minipal_get_cryptographically_secure_random_bytes(bytes,-1)==-1 && errno==EINVAL);
    error=1;assert(minipal_get_cryptographically_secure_random_bytes(bytes,8)==-1 && errno==EIO);
    absent=1;assert(minipal_get_cryptographically_secure_random_bytes(bytes,8)==-1 && errno==ENOSYS);
    puts("MINIPAL ENTROPY ADAPTER PASS exact success/error semantics and no fake entropy fallback");
}
