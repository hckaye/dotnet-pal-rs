"""Source-level ELF discovery adapter for the audited native runtime."""
from kernel_patch import once
from unwind_patch import FILE as LOCK
PAL='src/coreclr/nativeaot/Runtime/unix/PalUnix.cpp'
ADDRESS='src/native/external/llvm-libunwind/src/AddressSpace.hpp'
FILES=(ADDRESS,)
PRELUDE='''#ifdef DOTNET_PAL_ELF_METADATA
#include "elf_adapter.h"
#define DOTNET_PAL_ELF_ITERATE dotnet_pal_elf::iterate
#define DOTNET_PAL_ELF_LOOKUP dotnet_pal_elf::lookup
#else
#define DOTNET_PAL_ELF_ITERATE dl_iterate_phdr
#define DOTNET_PAL_ELF_LOOKUP dladdr
#endif
'''
def pal(text):
    # The original name lookup has already been routed by runtime_patch.
    if text.count('dl_iterate_phdr(')!=1: raise ValueError('expected one PAL ELF enumeration')
    text=text.replace('dl_iterate_phdr(', 'DOTNET_PAL_ELF_ITERATE(')
    return PRELUDE+text

def address(text):
    if text.count('dl_iterate_phdr(')!=1 or text.count('dladdr(')!=1:
        raise ValueError('expected unique libunwind ELF discovery calls')
    text=text.replace('dl_iterate_phdr(', 'DOTNET_PAL_ELF_ITERATE(').replace('dladdr(', 'DOTNET_PAL_ELF_LOOKUP(')
    # Select libunwind's existing ELF enumeration fallback. Calling an OS function
    # returned from dlsym outside the boundary would not establish OS isolation.
    anchor='#if defined(DLFO_STRUCT_HAS_EH_DBASE) && defined(_LIBUNWIND_SUPPORT_DWARF_INDEX) && DLFO_EH_SEGMENT_TYPE == PT_GNU_EH_FRAME'
    text=once(text,anchor,anchor+' && !defined(DOTNET_PAL_ELF_METADATA)')
    marker='namespace libunwind {\n\n/// Used by findUnwindSections()'
    return once(text,marker,PRELUDE+'\n'+marker)
