"""Guarded source integration of native helper services on the audited pin.

No global CRT interposition. Only paired allocator call sites in dump formatting
and libunwind are redirected; getline/asprintf and C++ allocation are untouched.
"""
import re
from kernel_patch import once
MARKER = "DOTNET_PAL_SUPPORT"
DUMP = "src/coreclr/nativeaot/Runtime/unix/PalCreateDump.cpp"
RW = "src/native/external/llvm-libunwind/src/RWMutex.hpp"
CONFIG = "src/native/external/llvm-libunwind/src/config.h"
CURSOR = "src/native/external/llvm-libunwind/src/UnwindCursor.hpp"
LIBUNWIND = "src/native/external/llvm-libunwind/src/libunwind.cpp"
FILES = (DUMP, RW, CONFIG, CURSOR, LIBUNWIND)


def function(text, name, body):
    pattern = re.compile(rf"(^[^\n]*\b{re.escape(name)}\([^)]*\)\n\{{\n)(.*?)(^\}})", re.M | re.S)
    if len(list(pattern.finditer(text))) != 1:
        raise ValueError(f"expected one pinned definition: {name}")
    return pattern.sub(lambda m: m[1] + f"#ifdef {MARKER}\n" + body + "\n#else\n" + m[2] + "#endif\n" + m[3], text)


def pal(text):
    text = once(text, "bool PalInit()\n{", "bool PalInit()\n{\n#ifdef DOTNET_PAL_SUPPORT\n    if (!dotnet_pal_support::initialize() || !dotnet_pal_unwind_initialize()) return false;\n#endif")
    text = function(text, "PalSetCurrentThreadName", "    if (PalGetCurrentOSThreadId() == PalGetCurrentProcessId()) return true;\n    return dotnet_pal_support::name(name);")
    text = function(text, "PalPrintFatalError", "    dotnet_pal_support::fatal_message(message);")
    text = function(text, "PalGetSystemTimeAsFileTime", "    uint64_t ticks = dotnet_pal_support::filetime();\n    lpSystemTimeAsFileTime->dwLowDateTime = static_cast<uint32_t>(ticks);\n    lpSystemTimeAsFileTime->dwHighDateTime = static_cast<uint32_t>(ticks >> 32);")
    constants = "static const int64_t SECS_BETWEEN_1601_AND_1970_EPOCHS = 11644473600LL;\nstatic const int64_t SECS_TO_100NS = 10000000; /* 10^7 */"
    text = once(text, constants, f"#ifndef {MARKER}\n" + constants + "\n#endif")
    return f'#ifdef {MARKER}\n#include "support_adapter.h"\nextern "C" bool dotnet_pal_unwind_initialize();\n#endif\n' + text


def gc(text):
    text = function(text, "GCToOSInterface::GetCurrentThreadIdForLogging", "    return dotnet_pal_runtime::identity(true);")
    text = function(text, "GCToOSInterface::GetCurrentProcessId", "    return static_cast<uint32_t>(dotnet_pal_runtime::identity(false));")
    return f'#ifdef {MARKER}\n#include "runtime_adapter.h"\n#endif\n' + text


def dump(text):
    if MARKER in text:
        raise ValueError("dump is already patched")
    counts = {}
    for old, new in (("malloc", "DOTNET_PAL_DUMP_ALLOC"), ("free", "DOTNET_PAL_DUMP_FREE"), ("getpid", "DOTNET_PAL_DUMP_PID")):
        text, count = re.subn(rf"\b{old}\s*\(", new + "(", text)
        counts[old] = count
    if counts != {"malloc": 3, "free": 8, "getpid": 1}:
        raise ValueError(f"dump call-site counts changed: {counts}")
    return '''#ifdef DOTNET_PAL_SUPPORT
#include "support_adapter.h"
#include "context_adapter.h"
#define DOTNET_PAL_DUMP_ALLOC dotnet_pal_support::allocate
#define DOTNET_PAL_DUMP_FREE dotnet_pal_support::release
#define DOTNET_PAL_DUMP_PID() static_cast<pid_t>(dotnet_pal_context::process_id_async())
#else
#define DOTNET_PAL_DUMP_ALLOC malloc
#define DOTNET_PAL_DUMP_FREE free
#define DOTNET_PAL_DUMP_PID getpid
#endif
''' + text


def rw(text):
    text = once(text, "#define __RWMUTEX_HPP__", '#define __RWMUTEX_HPP__\n#ifdef DOTNET_PAL_SUPPORT\n#include "support_adapter.h"\n#endif')
    return once(text, "#if defined(_LIBUNWIND_HAS_NO_THREADS)", "#if defined(DOTNET_PAL_SUPPORT)\nclass _LIBUNWIND_HIDDEN RWMutex : public dotnet_pal_support::ReadWriteLock {};\n#elif defined(_LIBUNWIND_HAS_NO_THREADS)")


def config(text):
    # Keep C/assembler inclusions and unrelated runtimes unchanged.
    return text + '''
#if defined(DOTNET_PAL_SUPPORT) && defined(__cplusplus)
#include "support_adapter.h"
#undef _LIBUNWIND_REMEMBER_ALLOC
#undef _LIBUNWIND_REMEMBER_FREE
#define _LIBUNWIND_REMEMBER_ALLOC(_size) dotnet_pal_support::allocate(_size)
#define _LIBUNWIND_REMEMBER_FREE(_ptr) dotnet_pal_support::release(_ptr)
#endif
'''


def cursor(text):
    text = once(text, "  static pint_t findFDE(pint_t mh, pint_t pc);",
                "  static pint_t findFDE(pint_t mh, pint_t pc);\n#ifdef DOTNET_PAL_SUPPORT\n  static bool initialize() { return _lock.initialize(); }\n#endif")
    for old, new in (("(entry *)malloc(newSize * sizeof(entry))", "(entry *)dotnet_pal_support::allocate(newSize * sizeof(entry))"), ("free(_buffer);", "dotnet_pal_support::release(_buffer);")):
        lines = [s for s in text.splitlines() if old in s]
        if len(lines) != 1:
            raise ValueError(f"missing/duplicate unwind allocator: {old}")
        original = lines[0]
        text = once(text, original, f"#ifdef {MARKER}\n" + original.replace(old, new) + "\n#else\n" + original + "\n#endif")
    allocation = "    entry *newBuffer = (entry *)dotnet_pal_support::allocate(newSize * sizeof(entry));"
    text = once(text, allocation, """    if (oldSize > SIZE_MAX / (4 * sizeof(entry))) {
      _LIBUNWIND_LOG_IF_FALSE(_lock.unlock());
      return; // Cache growth is optional; keep all previously cached entries.
    }
""" + allocation + """
    if (newBuffer == nullptr) {
      _LIBUNWIND_LOG_IF_FALSE(_lock.unlock());
      return; // An OOM must not memcpy through NULL or discard the old cache.
    }""")
    return text


def libunwind(text):
    return once(text, "using namespace libunwind;", """using namespace libunwind;
#ifdef DOTNET_PAL_SUPPORT
// NativeAOT calls this before installing signal handlers or starting threads.
extern "C" bool dotnet_pal_unwind_initialize() {
  return DwarfFDECache<LocalAddressSpace>::initialize();
}
#endif""")


TRANSFORMS = {DUMP: dump, RW: rw, CONFIG: config, CURSOR: cursor, LIBUNWIND: libunwind}
