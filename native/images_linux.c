/* Linux ELF64 reference provider. Loader lock scopes each metadata callback. */
#ifndef _GNU_SOURCE
#define _GNU_SOURCE
#endif
#include "dotnet_pal.h"
#include <dlfcn.h>
#include <link.h>
#include <string.h>
_Static_assert(sizeof(ElfW(Phdr))==56,"ELF64 headers required");
struct visit_context {dotnet_pal_image_visitor visitor;void *data;int failed;};
static int visit(struct dl_phdr_info *info,size_t size,void *data){
    struct visit_context *c=data;
    if(size<offsetof(struct dl_phdr_info,dlpi_subs)+sizeof(info->dlpi_subs)){c->failed=1;return 1;}
    dotnet_pal_image_view v={DOTNET_PAL_IMAGE_ELF64_LE,0,info->dlpi_addr,
        (const uint8_t*)info->dlpi_name,info->dlpi_name?strlen(info->dlpi_name):0,
        (const uint8_t*)info->dlpi_phdr,info->dlpi_phnum,info->dlpi_adds,info->dlpi_subs};
    return c->visitor(&v,c->data);
}
static uint32_t iterate(dotnet_pal_image_visitor visitor,void *data,int32_t *out){
    struct visit_context c={visitor,data,0};int result=dl_iterate_phdr(visit,&c);
    if(c.failed)return DOTNET_PAL_OS_ERROR;
    *out=result;return DOTNET_PAL_OK;
}
static uint32_t address_info(void *p,dotnet_pal_symbol_info *out){
    Dl_info d={0};if(!dladdr(p,&d))return DOTNET_PAL_NOT_FOUND;
    *out=(dotnet_pal_symbol_info){d.dli_fbase,(const uint8_t*)d.dli_fname,d.dli_fname?strlen(d.dli_fname):0,
        d.dli_saddr,(const uint8_t*)d.dli_sname,d.dli_sname?strlen(d.dli_sname):0};return DOTNET_PAL_OK;
}
static const dotnet_pal_host_images IMAGES={
    {DOTNET_PAL_ABI_VERSION,sizeof(dotnet_pal_host_images),DOTNET_PAL_CAP_IMAGES},
    {iterate,address_info,NULL}
};
const dotnet_pal_host_images *dotnet_pal_host_images_v2(void){return &IMAGES;}
