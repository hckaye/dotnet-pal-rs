#ifndef DOTNET_PAL_IMAGES_ADAPTER_H
#define DOTNET_PAL_IMAGES_ADAPTER_H
#include "dotnet_pal.h"
#include <atomic>
#include <dlfcn.h>
#include <link.h>
#if __BYTE_ORDER__ != __ORDER_LITTLE_ENDIAN__
#error "The ELF image adapter requires little endian metadata"
#endif
namespace dotnet_pal_images {
static_assert(sizeof(ElfW(Phdr))==56,"ELF64 adapter only");
inline std::atomic<const dotnet_pal_image_ops*> installed{nullptr};
inline bool initialize(){
    auto *a=dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if(!a || a->header.abi_version!=DOTNET_PAL_ABI_VERSION || a->header.struct_size<DOTNET_PAL_IMAGES_API_SIZE
        || (a->header.capabilities&DOTNET_PAL_CAP_IMAGES)!=DOTNET_PAL_CAP_IMAGES
        || !a->images.iterate || !a->images.address_info)return false;
    installed.store(&a->images,std::memory_order_release);return true;
}
using Callback=int(*)(dl_phdr_info*,size_t,void*);
struct Context {Callback callback;void *data;};
inline int32_t visit(const dotnet_pal_image_view *v,void *data){
    if(v->format!=DOTNET_PAL_IMAGE_ELF64_LE || v->header_count>UINT16_MAX)__builtin_trap();
    auto *c=static_cast<Context*>(data);
    dl_phdr_info info{};info.dlpi_addr=v->load_bias;
    info.dlpi_name=reinterpret_cast<const char*>(v->name);
    info.dlpi_phdr=reinterpret_cast<const ElfW(Phdr)*>(v->headers);
    info.dlpi_phnum=static_cast<ElfW(Half)>(v->header_count);
    info.dlpi_adds=v->added;info.dlpi_subs=v->removed;
    // Do not advertise TLS fields that the portable view does not provide.
    return c->callback(&info,offsetof(dl_phdr_info,dlpi_subs)+sizeof(info.dlpi_subs),c->data);
}
inline int iterate(Callback callback,void *data){
    auto *ops=installed.load(std::memory_order_acquire);if(!ops || !callback)__builtin_trap();
    Context context{callback,data};int32_t result=0;
    if(ops->iterate(visit,&context,&result)!=DOTNET_PAL_OK)__builtin_trap();
    return result;
}
inline int address(const void *address,Dl_info *out){
    auto *ops=installed.load(std::memory_order_acquire);if(!ops || !out)__builtin_trap();
    *out={};dotnet_pal_symbol_info info{};
    if(ops->address_info(const_cast<void*>(address),&info)!=DOTNET_PAL_OK)return 0;
    out->dli_fbase=info.base;out->dli_fname=reinterpret_cast<const char*>(info.name);
    out->dli_saddr=info.symbol_address;out->dli_sname=reinterpret_cast<const char*>(info.symbol_name);return 1;
}
}
#endif
