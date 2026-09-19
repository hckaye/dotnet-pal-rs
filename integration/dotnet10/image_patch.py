"""Route unwind-table lookup, readability probes, build ids and unwinder
diagnostics through the PAL image group.

libunwind keeps its DWARF parsing and caches; only the questions "which image
contains this address and where are its tables", "can this address be read"
and "print this line" leave the runtime.
"""
import re
from kernel_patch import once
from topology_patch import function

MARKER = "DOTNET_PAL_IMAGE"
ADDRESS_SPACE = "src/native/external/llvm-libunwind/src/AddressSpace.hpp"
CURSOR = "src/native/external/llvm-libunwind/src/UnwindCursor.hpp"
CONFIG = "src/native/external/llvm-libunwind/src/config.h"
PAL = "src/coreclr/nativeaot/Runtime/unix/PalUnix.cpp"
FILES = (ADDRESS_SPACE, CURSOR)

LOOKUP = f"""#ifdef {MARKER}
  dotnet_pal_unwind_info pal;
  if (!dotnet_pal_image::unwind(targetAddr, pal))
    return false;
  info.dso_base = pal.text_start;
  info.text_segment_length = pal.text_length;
  if (pal.eh_frame_hdr != 0) {{
    info.dwarf_index_section = pal.eh_frame_hdr;
    info.dwarf_index_section_length = pal.eh_frame_hdr_length;
    EHHeaderParser<LocalAddressSpace>::EHHeaderInfo hdrInfo;
    if (!EHHeaderParser<LocalAddressSpace>::decodeEHHdr(
            *this, info.dwarf_index_section,
            info.dwarf_index_section + info.dwarf_index_section_length, hdrInfo))
      return false;
    info.dwarf_section = hdrInfo.eh_frame_ptr;
    info.dwarf_section_length = SIZE_MAX;
    return true;
  }}
  if (pal.eh_frame == 0)
    return false;
  info.dwarf_section = pal.eh_frame;
  info.dwarf_section_length = pal.eh_frame_length;
  return true;
#else"""


def address_space(text):
    if MARKER in text:
        raise ValueError("address space is already patched")
    text = once(text, "#elif defined(_LIBUNWIND_USE_DL_ITERATE_PHDR)\n  // Use DLFO_STRUCT_HAS_EH_DBASE to determine the existence of\n",
                "#elif defined(_LIBUNWIND_USE_DL_ITERATE_PHDR)\n" + LOOKUP + "\n  // Use DLFO_STRUCT_HAS_EH_DBASE to determine the existence of\n")
    text = once(text, "  int found = dl_iterate_phdr(findUnwindSectionsByPhdr, &cb_data);\n  return static_cast<bool>(found);\n#endif\n",
                f"  int found = dl_iterate_phdr(findUnwindSectionsByPhdr, &cb_data);\n  return static_cast<bool>(found);\n#endif // {MARKER}\n#endif\n")
    # The program-header walkers serve only the replaced path.
    text = once(text, "#if defined(_LIBUNWIND_USE_DL_ITERATE_PHDR)\n\n// The ElfW() macro",
                f"#if defined(_LIBUNWIND_USE_DL_ITERATE_PHDR) && !defined({MARKER})\n\n// The ElfW() macro")
    text = once(text, "#if _LIBUNWIND_USE_DLADDR\n  Dl_info dyldInfo;", f"#if _LIBUNWIND_USE_DLADDR && !defined({MARKER})\n  Dl_info dyldInfo;")
    return once(text, "namespace libunwind {\n\n/// Used by findUnwindSections()",
                f'#ifdef {MARKER}\n#include "image_adapter.h"\n#endif\nnamespace libunwind {{\n\n/// Used by findUnwindSections()')


def cursor(text):
    old = "  const auto sigsetAddr = reinterpret_cast<sigset_t *>(addr);"
    regex = re.compile(re.escape(old) + r".*?\n  const auto readable = errno != EFAULT;\n  errno = saveErrno;\n  return readable;\n", re.S)
    if len(list(regex.finditer(text))) != 1:
        raise ValueError("expected one sigreturn readability probe")
    return regex.sub(lambda m: f"#ifdef {MARKER}\n  return addr != 0 && dotnet_pal_image::readable(addr, NSIG / 8);\n#else\n" + m[0] + "#endif\n", text)


def config(text):
    """config.h after the support patch appended its allocator overrides."""
    return text + f'''
#if defined({MARKER}) && defined(__cplusplus)
#include "image_adapter.h"
#undef _LIBUNWIND_ABORT
#define _LIBUNWIND_ABORT(msg)                                                  \\
  do {{                                                                         \\
    dotnet_pal_image::log("libunwind: %s - %s", __func__, msg);                \\
    abort();                                                                   \\
  }} while (0)
#undef _LIBUNWIND_LOG0
#define _LIBUNWIND_LOG0(msg) dotnet_pal_image::log("libunwind: " msg)
#undef _LIBUNWIND_LOG
#define _LIBUNWIND_LOG(msg, ...) dotnet_pal_image::log("libunwind: " msg, __VA_ARGS__)
#ifndef NDEBUG
#undef _LIBUNWIND_TRACE_DWARF
#define _LIBUNWIND_TRACE_DWARF(...)                                            \\
  do {{                                                                         \\
    if (logDWARF())                                                            \\
      dotnet_pal_image::log(__VA_ARGS__);                                      \\
  }} while (0)
#endif
#endif
'''


PDB = """    memset(pGuidSignature, 0, sizeof(*pGuidSignature));
    *pdwAge = 0;
    *ppBuildId = NULL;
    *pcbBuildId = 0;
    if (cchPath <= 0)
        return;
    wszPath[0] = L'\\0';
    const void* data = nullptr;
    uint32_t length = 0;
    if (dotnet_pal_image::build_id(reinterpret_cast<uintptr_t>(hOsHandle), data, length))
    {
        *ppBuildId = const_cast<void*>(data);
        *pcbBuildId = length;
    }"""


def pal(text):
    """PalUnix.cpp: build ids through the image group; negotiate at startup."""
    text = once(text, "bool PalInit()\n{", f"bool PalInit()\n{{\n#ifdef {MARKER}\n    if (!dotnet_pal_image::initialize()) return false;\n#endif")
    text = function(text, "PalGetPDBInfo", PDB, MARKER)
    text = once(text, "#if TARGET_LINUX\n\nstruct PalGetPDBInfoPhdrCallbackData", f"#if TARGET_LINUX && !defined({MARKER})\n\nstruct PalGetPDBInfoPhdrCallbackData")
    return f'#ifdef {MARKER}\n#include "image_adapter.h"\n#endif\n' + text


# UnwindCursor.hpp is composed after the support patch in patch_runtime.py.
TRANSFORMS = {ADDRESS_SPACE: address_space}
