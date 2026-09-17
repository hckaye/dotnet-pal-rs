"""ELF image enumeration and symbolization, without a raw loader-API escape hatch."""
import re
from kernel_patch import once
from support_patch import DUMP
MARKER='DOTNET_PAL_IMAGES'
ADDRESS='src/native/external/llvm-libunwind/src/AddressSpace.hpp'
FILES=(ADDRESS,)


def callsites(text, old, new, expected):
    # Pinned call expressions only; no global libc symbol interposition.
    matches=list(re.finditer(r'\b'+re.escape(old)+r'\s*\(',text))
    if len(matches)!=expected:
        raise ValueError(f'{old}: expected {expected} pinned call sites, found {len(matches)}')
    name='DOTNET_PAL_IMAGE_'+old.upper()
    text=re.sub(r'\b'+re.escape(old)+r'\s*\(',name+'(',text)
    return f'#ifdef {MARKER}\n#include "images_adapter.h"\n#define {name} {new}\n#else\n#define {name} {old}\n#endif\n'+text


def pal(text):
    text=once(text,'bool PalInit()\n{','bool PalInit()\n{\n#ifdef DOTNET_PAL_IMAGES\n    if (!dotnet_pal_images::initialize()) return false;\n#endif')
    return callsites(text,'dl_iterate_phdr','dotnet_pal_images::iterate',1)


def dump(text):
    return callsites(text,'dladdr','dotnet_pal_images::address',1)


def address(text):
    fastpath='#if defined(DLFO_STRUCT_HAS_EH_DBASE) && defined(_LIBUNWIND_SUPPORT_DWARF_INDEX) && DLFO_EH_SEGMENT_TYPE == PT_GNU_EH_FRAME'
    text=once(text,fastpath,fastpath+' && !defined(DOTNET_PAL_IMAGES)')
    # _dl_find_object bypasses the portable image contract via a dlsym-obtained
    # function pointer. Use the existing, fully qualified program-header path.
    text=callsites(text,'dl_iterate_phdr','dotnet_pal_images::iterate',1)
    return callsites(text,'dladdr','dotnet_pal_images::address',1)


TRANSFORMS={ADDRESS:address}
