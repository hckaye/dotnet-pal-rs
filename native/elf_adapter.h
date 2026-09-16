#ifndef DOTNET_PAL_ELF_ADAPTER_H
#define DOTNET_PAL_ELF_ADAPTER_H
#include "dotnet_pal.h"
#include <dlfcn.h>
#include <link.h>
#include <cstddef>
#include <cstring>
#include <limits>
static_assert(sizeof(ElfW(Phdr)) == sizeof(dotnet_pal_elf64_header), "ELF64 target required");
static_assert(offsetof(ElfW(Phdr), p_type) == offsetof(dotnet_pal_elf64_header, type));
static_assert(offsetof(ElfW(Phdr), p_flags) == offsetof(dotnet_pal_elf64_header, flags));
static_assert(offsetof(ElfW(Phdr), p_offset) == offsetof(dotnet_pal_elf64_header, offset));
static_assert(offsetof(ElfW(Phdr), p_vaddr) == offsetof(dotnet_pal_elf64_header, virtual_address));
static_assert(offsetof(ElfW(Phdr), p_paddr) == offsetof(dotnet_pal_elf64_header, physical_address));
static_assert(offsetof(ElfW(Phdr), p_filesz) == offsetof(dotnet_pal_elf64_header, file_size));
static_assert(offsetof(ElfW(Phdr), p_memsz) == offsetof(dotnet_pal_elf64_header, memory_size));
static_assert(offsetof(ElfW(Phdr), p_align) == offsetof(dotnet_pal_elf64_header, alignment));
namespace dotnet_pal_elf {
inline const dotnet_pal_elf_ops *ops() {
    const auto *p = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    if (!p || p->header.abi_version != DOTNET_PAL_ABI_VERSION || p->header.struct_size < DOTNET_PAL_ELF_API_SIZE ||
        !(p->header.capabilities & DOTNET_PAL_CAP_ELF64_METADATA) || !p->elf.enumerate || !p->elf.lookup) return nullptr;
    return &p->elf;
}
struct Visit { int (*callback)(struct dl_phdr_info*,size_t,void*); void *data; };
inline int32_t visit(const dotnet_pal_elf_image *image, void *data) {
    auto *v = static_cast<Visit*>(data);
    // Adapter-local SDK structure. No caller sees the provider's dl_phdr_info.
    struct dl_phdr_info info{};
    info.dlpi_addr = image->load_bias;
    info.dlpi_name = image->name;
    info.dlpi_phdr = reinterpret_cast<const ElfW(Phdr)*>(image->headers);
    info.dlpi_phnum = static_cast<ElfW(Half)>(image->header_count);
    size_t size = offsetof(struct dl_phdr_info, dlpi_adds);
    if (image->flags & DOTNET_PAL_ELF_LOAD_COUNTERS) {
        info.dlpi_adds = image->loads; info.dlpi_subs = image->unloads;
        // TLS metadata is not supplied by this capability; do not advertise it.
        size = offsetof(struct dl_phdr_info, dlpi_tls_modid);
    }
    return v->callback(&info,size,v->data);
}
inline int iterate(int (*callback)(struct dl_phdr_info*,size_t,void*),void *data) {
    const auto *p = ops();
    if (!p || !callback) return 0;
    Visit v{callback,data}; int32_t result = 0;
    return p->enumerate(visit,&v,&result) == DOTNET_PAL_OK ? result : 0;
}
inline int lookup(const void *address, Dl_info *out) {
    const auto *p = ops(); if (!out) return 0;
    *out = {};
    dotnet_pal_elf_symbol result{};
    if (!p || p->lookup(address,&result) != DOTNET_PAL_OK) return 0;
    out->dli_fname=result.module_name; out->dli_fbase=result.module_base;
    out->dli_sname=result.symbol_name; out->dli_saddr=result.symbol_address;
    return 1;
}
}
#endif
