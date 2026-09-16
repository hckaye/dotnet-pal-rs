"""Audited, narrowly guarded edits for native synchronization/thread/TLS paths.

The caller validates the exact upstream revision and clean target files before
using any transformer. Missing/duplicate anchors reject the whole preparation.
"""
import re

MARKER = "DOTNET_PAL_KERNEL"
GC_HEADER = "src/coreclr/gc/env/gcenv.os.h"
GC_EVENTS = "src/coreclr/gc/unix/events.cpp"
PAL = "src/coreclr/nativeaot/Runtime/unix/PalUnix.cpp"
CRST_HEADER = "src/coreclr/nativeaot/Runtime/Crst.h"
CRST_SOURCE = "src/coreclr/nativeaot/Runtime/Crst.cpp"
FILES = (GC_HEADER, GC_EVENTS, PAL, CRST_HEADER, CRST_SOURCE)


def once(text, old, new):
    if text.count(old) != 1:
        raise ValueError(f"expected one pinned anchor: {old[:90]!r}")
    return text.replace(old, new, 1)


def guarded(original, replacement):
    return f"#ifdef {MARKER}\n{replacement}\n#else\n{original}\n#endif"


def function(text, name, body):
    # Pinned top-level definitions start/finish at column zero.
    regex = re.compile(rf"(^[^\n]*\b{re.escape(name)}\([^\n]*\)\n\{{\n)(.*?)(^\}})", re.M | re.S)
    if len(list(regex.finditer(text))) != 1:
        raise ValueError(f"expected one definition of {name}")
    return regex.sub(lambda m: m[1] + guarded(m[2].rstrip(), body) + "\n" + m[3], text)


def gc_extra(text):
    start = text.index("bool CanFlushUsingMembarrier()")
    # Only the membarrier helper and its state are removed. Serviced runtimes
    # declare g_configuredCpuCount immediately afterwards; it must remain live.
    state = "static pthread_mutex_t g_flushProcessWriteBuffersMutex;"
    if text.count(state) != 1: raise ValueError("missing or duplicate flush state")
    end = text.index(state, start) + len(state)

    text = text[:start] + f"#ifndef {MARKER}\n" + text[start:end] + "\n#endif\n\n" + text[end:]
    anchor = "#ifndef TARGET_WASM\n    assert(s_flushUsingMemBarrier == 0);"
    text = once(text, anchor, f"#if !defined(TARGET_WASM) && !defined({MARKER})\n    assert(s_flushUsingMemBarrier == 0);")
    old = "    int pageSize = sysconf( _SC_PAGE_SIZE );"
    new = "    if (!dotnet_pal_kernel::api() || !dotnet_pal_gc::api()) return false;\n    int pageSize = static_cast<int>(dotnet_pal_gc::api()->vm.page_size());"
    text = once(text, old, guarded(old, new))
    text = function(text, "GCToOSInterface::FlushProcessWriteBuffers", "    dotnet_pal_kernel::barrier();")
    text = function(text, "GCToOSInterface::Shutdown", "    CleanupCGroup();")
    return f'#ifdef {MARKER}\n#include "kernel_adapter.h"\n#endif\n' + text


def header(text):
    regex = re.compile(r"class CLRCriticalSection final\n\{.*?^\};", re.M | re.S)
    matches = list(regex.finditer(text))
    if len(matches) != 1: raise ValueError("expected one CLRCriticalSection")
    replacement = '''#include "kernel_adapter.h"
class CLRCriticalSection final {
    void *m_cs = nullptr;
public:
    bool Initialize() { return dotnet_pal_kernel::mutex_init(&m_cs); }
    void Destroy() { dotnet_pal_kernel::mutex_destroy(m_cs); m_cs = nullptr; }
    void Enter() { dotnet_pal_kernel::mutex_lock(m_cs); }
    void Leave() { dotnet_pal_kernel::mutex_unlock(m_cs); }
};'''
    return regex.sub(lambda m: guarded(m[0], replacement), text)


def events(text):
    if "class GCEvent::Impl" not in text: raise ValueError("expected GCEvent implementation")
    return guarded(text, '#include "gc_events_adapter.inl"') + "\n"


def crst_header(text):
    return once(text, "    minipal_mutex    m_Lock;", guarded("    minipal_mutex    m_Lock;", "    void* m_Lock;"))


def crst_source(text):
    edits = {
        "    minipal_mutex_init(&m_Lock);": "    dotnet_pal_kernel::mutex_init_or_abort(&m_Lock);",
        "    minipal_mutex_destroy(&m_Lock);": "    dotnet_pal_kernel::mutex_destroy(m_Lock);",
        "    minipal_mutex_enter(&pCrst->m_Lock);": "    dotnet_pal_kernel::mutex_lock(pCrst->m_Lock);",
        "    minipal_mutex_leave(&pCrst->m_Lock);": "    dotnet_pal_kernel::mutex_unlock(pCrst->m_Lock);",
    }
    for old, new in edits.items(): text = once(text, old, guarded(old, new))
    return f'#ifdef {MARKER}\n#include "kernel_adapter.h"\n#endif\n' + text


def pal(text):
    start = text.index("static void TimeSpecAdd(")
    end = text.index("// This functions configures behavior of the signals", start)
    text = text[:start] + guarded(text[start:end], '#include "runtime_events_adapter.inl"') + "\n\n" + text[end:]
    text = once(text, "static pthread_key_t key;", guarded("static pthread_key_t key;", "static void* key;"))
    text = once(text, "bool PalInit()\n{", "bool PalInit()\n{\n#ifdef DOTNET_PAL_KERNEL\n    if (!dotnet_pal_kernel::api() || !dotnet_pal_gc::api() || !dotnet_pal_gc_services::api()) return false;\n#endif")
    old = "    if (pthread_key_create(&key, RuntimeThreadShutdown) != 0)"
    text = once(text, old, guarded(old, "    if (dotnet_pal_kernel::require()->tls_create(RuntimeThreadShutdown, &key) != DOTNET_PAL_OK)"))
    old = "    if (pthread_setspecific(key, thread) != 0)"
    text = once(text, old, guarded(old, "    if (dotnet_pal_kernel::require()->tls_set(key, thread) != DOTNET_PAL_OK)"))
    text = function(text, "PalStartBackgroundWork", "    (void)highPriority;\n    return dotnet_pal_kernel::start_background(callback, pCallbackContext, GetDefaultStackSizeSetting());")
    text = function(text, "PalGetMaximumStackBounds", "    return dotnet_pal_kernel::require()->stack_bounds(ppStackLowOut, ppStackHighOut) == DOTNET_PAL_OK;")
    text = function(text, "PalSleep", "    dotnet_pal_gc_services::sleep_ms(milliseconds);")
    text = function(text, "PalSwitchToThread", "    dotnet_pal_gc_services::yield_thread(0);\n    return false;")
    return f'#ifdef {MARKER}\n#include "kernel_adapter.h"\n#include "gc_services_adapter.h"\n#include "gc_vm_adapter.h"\n#endif\n' + text


TRANSFORMS = {GC_HEADER: header, GC_EVENTS: events, PAL: pal, CRST_HEADER: crst_header, CRST_SOURCE: crst_source}
