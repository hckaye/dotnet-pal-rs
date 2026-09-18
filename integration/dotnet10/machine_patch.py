"""Guarded neutral machine measurements and CPU placement on the pinned runtime.

Preserve cgroup quota/memory-limit policy and environment overrides. Native OS
structs and selector constants do not become part of the public Rust ABI.
"""
import re
from kernel_patch import once
CGROUP = "src/coreclr/gc/unix/cgroup.cpp"
FILES = (CGROUP,)
MARKER = "DOTNET_PAL_MACHINE"
def guarded(old, new):
    return f"#ifdef {MARKER}\n{new}\n#else\n{old}\n#endif"
def line(text, old, new):
    return once(text, old, guarded(old, new))
def function(text, name, body):
    pattern = re.compile(rf"(^[^\n]*\b{re.escape(name)}\([^)]*\)\n\{{\n)(.*?)(^\}})", re.M | re.S)
    if len(list(pattern.finditer(text))) != 1: raise ValueError(f"expected one pinned definition: {name}")
    return pattern.sub(lambda m: m[1] + guarded(m[2].rstrip(), body) + '\n' + m[3], text)
def prefix(text):
    return f'#ifdef {MARKER}\n#include "machine_adapter.h"\n#endif\n' + text
def gc(text):
    text = line(text, '    int cpuCount = sysconf(SYSCONF_GET_NUMPROCS);',
        '    int cpuCount = static_cast<int>(dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_ONLINE_CPUS));')
    text = line(text, '    int configuredCpuCount = minipal_get_cpu_max_possible_count();',
        '    int configuredCpuCount = static_cast<int>(dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_POSSIBLE_CPUS));')
    start = text.index('#if HAVE_SCHED_GETAFFINITY\n')
    end = text.index('#endif // HAVE_SCHED_GETAFFINITY', start) + len('#endif // HAVE_SCHED_GETAFFINITY')
    text = text[:start] + guarded(text[start:end], '    if (!dotnet_pal_machine::initialize_affinity(g_processAffinitySet, configuredCpuCount)) return false;') + text[end:]
    replacements = {
        '    long pages = sysconf(_SC_PHYS_PAGES);': '    long pages = dotnet_pal_machine::physical_pages();',
        '        long size = sysconf(cacheLevelNames[i]);': '        long size = dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_CACHE_L1 + i);',
        '            long pageSize = sysconf(_SC_PAGE_SIZE);': '            long pageSize = dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_PAGE_BYTES);',
        '        available = sysconf(SYSCONF_PAGES) * sysconf(_SC_PAGE_SIZE);': '        available = dotnet_pal_machine::available_bytes();',
        '    if ((getrlimit(RLIMIT_AS, &addressSpaceLimit) == 0) && (addressSpaceLimit.rlim_cur != RLIM_INFINITY))': '    if ((dotnet_pal_machine::read_limit(&addressSpaceLimit.rlim_cur) == 0) && (addressSpaceLimit.rlim_cur != RLIM_INFINITY))',
        '            if ((getrlimit(RLIMIT_AS, &addressSpaceLimit) == 0) && (addressSpaceLimit.rlim_cur != RLIM_INFINITY))': '            if ((dotnet_pal_machine::read_limit(&addressSpaceLimit.rlim_cur) == 0) && (addressSpaceLimit.rlim_cur != RLIM_INFINITY))',
    }
    # Exact whole-line matching: the two getrlimit sites have different indentation.
    for old, new in replacements.items():
        text = once(text, '\n'+old+'\n', '\n'+guarded(old,new)+'\n')
    text = function(text, 'GCToOSInterface::SetThreadAffinity', '    return dotnet_pal_machine::bind_current(procNo);')
    text = function(text, 'GCToOSInterface::GetCurrentProcessorNumber', '    return dotnet_pal_machine::current_cpu();')
    text = function(text, 'GCToOSInterface::CanGetCurrentProcessorNumber', '    return dotnet_pal_machine::api() != nullptr;')
    text = function(text, 'GetAvailablePageFile', '    return dotnet_pal_machine::swap_bytes();')
    return prefix(text)
def pal(text):
    text = once(text, 'bool PalInit()\n{', 'bool PalInit()\n{\n#ifdef DOTNET_PAL_MACHINE\n    if (!dotnet_pal_machine::api()) return false;\n#endif')
    start = text.index('#if HAVE_SCHED_GETAFFINITY\n', text.index('void InitializeCurrentProcessCpuCount()'))
    end = text.index('#endif // HAVE_SCHED_GETAFFINITY', start) + len('#endif // HAVE_SCHED_GETAFFINITY')
    body = '        count = dotnet_pal_machine::affinity_count();\n        if (count == 0) count = GCToOSInterface::GetTotalProcessorCount();'
    text = text[:start] + guarded(text[start:end], body) + text[end:]
    return prefix(text)
def cgroup(text):
    replacements = {
        '    if (getrlimit(RLIMIT_AS, &curr_rlimit) == 0)': '    if (dotnet_pal_machine::read_limit(&curr_rlimit.rlim_cur) == 0)',
        '    long pages = sysconf(_SC_PHYS_PAGES);': '    long pages = dotnet_pal_machine::physical_pages();',
        '        long pageSize = sysconf(_SC_PAGE_SIZE);': '        long pageSize = dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_PAGE_BYTES);',
        '            long pageSize = sysconf(_SC_PAGE_SIZE);': '            long pageSize = dotnet_pal_machine::long_query(DOTNET_PAL_MACHINE_PAGE_BYTES);',
    }
    for old,new in replacements.items(): text = once(text,'\n'+old+'\n','\n'+guarded(old,new)+'\n')
    return prefix(text)
TRANSFORMS = {CGROUP: cgroup}
