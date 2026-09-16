/* Independent ELF host provider; no native SDK struct crosses root ABI. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <dlfcn.h>
#include <link.h>
#include <stddef.h>
#include <string.h>
_Static_assert(sizeof(Elf64_Phdr)==sizeof(dotnet_pal_elf64_header), "ELF64 ABI");
#ifdef PAL_ELF_FAULT_HOST
extern int pal_elf_fault;
#else
#define pal_elf_fault 0
#endif
struct Visit {dotnet_pal_elf_visitor callback; void *data;};
static int visit(struct dl_phdr_info *info,size_t size,void *data) {
    struct Visit *v=data;
    dotnet_pal_elf_image image={info->dlpi_addr,info->dlpi_name,
        (const dotnet_pal_elf64_header*)info->dlpi_phdr,info->dlpi_phnum,0,0,0};
    if (size>=offsetof(struct dl_phdr_info,dlpi_subs)+sizeof info->dlpi_subs) {
        image.flags=DOTNET_PAL_ELF_LOAD_COUNTERS;image.loads=info->dlpi_adds;image.unloads=info->dlpi_subs;
    }
    return v->callback(&image,v->data);
}
static uint32_t enumerate(dotnet_pal_elf_visitor callback,void *data,int32_t *result) {
    if (pal_elf_fault==6 || pal_elf_fault==9) {
        dotnet_pal_elf_image bad={0,"",NULL,1,0,0,0};
        if(pal_elf_fault==9){bad.header_count=0;bad.flags=128;}
        *result=callback(&bad,data);return DOTNET_PAL_OK;
    }
    struct Visit v={callback,data};*result=dl_iterate_phdr(visit,&v);
    if(pal_elf_fault==7)*result=0;
    if(pal_elf_fault==8){dotnet_pal_elf_image next={0,"",NULL,0,0,0,0};callback(&next,data);}
    return pal_elf_fault==11 ? 999 : DOTNET_PAL_OK;
}
static uint32_t lookup(const void *address,dotnet_pal_elf_symbol *out) {
    if(pal_elf_fault==5 || pal_elf_fault==10 || pal_elf_fault==11){
        *out=(dotnet_pal_elf_symbol){NULL,(const char*)1,NULL,NULL};
        return pal_elf_fault==5 ? DOTNET_PAL_OS_ERROR : pal_elf_fault==11 ? 999 : DOTNET_PAL_OK;
    }
    Dl_info info={0};
    if(!dladdr(address,&info))return DOTNET_PAL_NOT_FOUND;
    *out=(dotnet_pal_elf_symbol){info.dli_fbase,info.dli_fname,info.dli_saddr,info.dli_sname};
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_elf HOST={{2,sizeof HOST,DOTNET_PAL_CAP_ELF64_METADATA},{enumerate,lookup,NULL}};
const dotnet_pal_host_elf *dotnet_pal_host_elf_v2(void){
    static dotnet_pal_host_elf bad;
    if(pal_elf_fault>=1 && pal_elf_fault<=4){
        bad=HOST;
        if(pal_elf_fault==1)bad.header.abi_version=999;
        if(pal_elf_fault==2)bad.header.struct_size=sizeof(dotnet_pal_header);
        if(pal_elf_fault==3)bad.header.capabilities=0;
        if(pal_elf_fault==4)bad.ops.enumerate=NULL;
        return &bad;
    }
    return &HOST;
}
