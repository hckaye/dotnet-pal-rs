/* Independent ELF reference provider for the host-image conformance suite.
 * Fault 1 withholds the table; fault 2 answers with impossible tables and errors. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <elf.h>
#include <errno.h>
#include <link.h>
#include <string.h>
#include <sys/syscall.h>
#include <unistd.h>
int pal_image_fault;
struct search { uintptr_t address; dotnet_pal_unwind_info *out; const uint8_t *id; size_t id_length; };
static int unwind_callback(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    struct search *s = data;
    const ElfW(Phdr) *text = NULL, *hdr = NULL;
    for (int i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *p = &info->dlpi_phdr[i];
        uintptr_t start = info->dlpi_addr + p->p_vaddr;
        if (p->p_type == PT_LOAD && s->address >= start && s->address < start + p->p_memsz) text = p;
        if (p->p_type == PT_GNU_EH_FRAME) hdr = p;
    }
    if (!text || !hdr) return 0;
    s->out->base = info->dlpi_addr;
    s->out->text_start = info->dlpi_addr + text->p_vaddr; s->out->text_length = text->p_memsz;
    s->out->eh_frame_hdr = info->dlpi_addr + hdr->p_vaddr; s->out->eh_frame_hdr_length = hdr->p_memsz;
    return 1;
}
static uint32_t unwind_info(uintptr_t address, dotnet_pal_unwind_info *out, size_t size) {
    if (size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    memset(out, 0, sizeof *out);
    if (pal_image_fault == 2) { out->text_start = address + 16; out->text_length = 4; out->eh_frame_hdr = 1; out->eh_frame_hdr_length = 1; return DOTNET_PAL_OK; }
    struct search s = {address, out, NULL, 0};
    return dl_iterate_phdr(unwind_callback, &s) ? DOTNET_PAL_OK : DOTNET_PAL_NOT_FOUND;
}
static uint32_t readable(uintptr_t address, size_t size) {
    if (pal_image_fault == 2) return DOTNET_PAL_OS_ERROR;
    long page = sysconf(_SC_PAGESIZE);
    for (uintptr_t probe = address & ~((uintptr_t)page - 1); probe < address + size; probe += (uintptr_t)page) {
        long rc = syscall(SYS_rt_sigprocmask, -1, (void*)probe, NULL, (size_t)8);
        if (rc == 0) return DOTNET_PAL_OS_ERROR;
        if (errno == EFAULT) return DOTNET_PAL_NOT_FOUND;
        if (errno != EINVAL) return DOTNET_PAL_OS_ERROR;
    }
    return DOTNET_PAL_OK;
}
static int note_callback(struct dl_phdr_info *info, size_t size, void *data) {
    (void)size;
    struct search *s = data;
    const ElfW(Phdr) *first = NULL;
    for (int i = 0; i < info->dlpi_phnum && !first; ++i) if (info->dlpi_phdr[i].p_type == PT_LOAD) first = &info->dlpi_phdr[i];
    if (!first || info->dlpi_addr + first->p_vaddr != s->address) return 0;
    for (int i = 0; i < info->dlpi_phnum; ++i) {
        const ElfW(Phdr) *p = &info->dlpi_phdr[i];
        if (p->p_type != PT_NOTE) continue;
        uintptr_t start = info->dlpi_addr + p->p_vaddr, end = start + p->p_memsz;
        size_t align = p->p_align > 4 ? p->p_align : 4;
        while (start + sizeof(ElfW(Nhdr)) <= end) {
            const ElfW(Nhdr) *note = (const ElfW(Nhdr)*)start;
            const uint8_t *name = (const uint8_t*)(note + 1);
            size_t namesz = (note->n_namesz + align - 1) & ~(align - 1);
            if (note->n_type == NT_GNU_BUILD_ID && note->n_namesz == 4 && memcmp(name, "GNU", 4) == 0) { s->id = name + namesz; s->id_length = note->n_descsz; return 1; }
            start += sizeof(ElfW(Nhdr)) + namesz + ((note->n_descsz + align - 1) & ~(align - 1));
        }
    }
    return 0;
}
static uint32_t build_id(uintptr_t base, uint8_t *out, size_t capacity, size_t *needed) {
    *needed = 0;
    if (pal_image_fault == 2) { if (capacity) out[0] = 1; return DOTNET_PAL_OS_ERROR; }
    struct search s = {base, NULL, NULL, 0};
    if (!dl_iterate_phdr(note_callback, &s) || s.id_length == 0) return DOTNET_PAL_NOT_FOUND;
    *needed = s.id_length;
    memcpy(out, s.id, s.id_length < capacity ? s.id_length : capacity);
    return s.id_length > capacity ? DOTNET_PAL_BUFFER_TOO_SMALL : DOTNET_PAL_OK;
}
static const dotnet_pal_host_image table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_image), DOTNET_PAL_CAP_IMAGE},
    {unwind_info, readable, build_id, NULL},
};
static const dotnet_pal_host_image malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_image), DOTNET_PAL_CAP_IMAGE}, {unwind_info, NULL, build_id, NULL}};
const dotnet_pal_host_image *dotnet_pal_host_image_v2(void) { return pal_image_fault == 1 ? &malformed : &table; }
