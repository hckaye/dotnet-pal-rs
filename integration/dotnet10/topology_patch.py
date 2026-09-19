"""Route CPU counts, affinity, memory accounting and cache size through the PAL.

The cgroup, cgroup CPU and NUMA readers of the pinned runtime are compiled out:
the topology group reports container limits already applied, and the boundary
exposes no NUMA placement (every consumer sees one node). The GC's per-CPU
cache heuristic stays in the runtime because it needs no OS.
"""
import re
from kernel_patch import once

MARKER = "DOTNET_PAL_TOPOLOGY"
GC = "src/coreclr/gc/unix/gcenv.unix.cpp"
CGROUP = "src/coreclr/gc/unix/cgroup.cpp"
CGROUP_CPU = "src/coreclr/nativeaot/Runtime/unix/cgroupcpu.cpp"
NUMA = "src/coreclr/gc/unix/numasupport.cpp"
PAL = "src/coreclr/nativeaot/Runtime/unix/PalUnix.cpp"
FILES = (CGROUP, CGROUP_CPU, NUMA)


def guarded(original, replacement, marker=MARKER):
    return f"#ifdef {marker}\n{replacement}\n#else\n{original}\n#endif"


def function(text, name, body, marker=MARKER):
    """Guards the single pinned definition of a function whose brace opens on its own line."""
    regex = re.compile(rf"(^[^\n]*\b{re.escape(name)}\([^\n]*\)\n\{{\n)(.*?)(^\}})", re.M | re.S)
    if len(list(regex.finditer(text))) != 1:
        raise ValueError(f"expected one definition of {name}")
    return regex.sub(lambda m: m[1] + guarded(m[2].rstrip(), body, marker) + "\n" + m[3], text)


def compiled_out(text, what):
    """Keeps a source file's OS readers only outside the PAL build."""
    if MARKER in text:
        raise ValueError(f"{what} is already patched")
    return f"#ifdef {MARKER}\n// {what}: replaced by the PAL topology group.\ntypedef int dotnet_pal_topology_replaces_{what};\n#else\n" + text + f"\n#endif // {MARKER}\n"


def cgroup(text): return compiled_out(text, "cgroup")
def cgroup_cpu(text): return compiled_out(text, "cgroupcpu")
def numa(text): return compiled_out(text, "numasupport")


INITIALIZE = """    if (!dotnet_pal_kernel::api() || !dotnet_pal_gc::api() || !dotnet_pal_topology::initialize()) return false;
    int pageSize = static_cast<int>(dotnet_pal_gc::api()->vm.page_size());
    g_pageSizeUnixInl = uint32_t((pageSize > 0) ? pageSize : 0x1000);
    uint32_t configuredCpuCount = dotnet_pal_topology::cpu_max();
    g_totalCpuCount = configuredCpuCount;
    g_configuredCpuCount = configuredCpuCount;
    if (!g_processAffinitySet.Initialize(configuredCpuCount)) return false;
    if (!dotnet_pal_topology::affinity(configuredCpuCount, [](uint32_t cpu) { g_processAffinitySet.Add(cpu); })) return false;
    if (g_processAffinitySet.Count() == 0) return false;
    uint32_t currentCpu = 0;
    g_palCanGetCurrentProcessor = dotnet_pal_topology::current_cpu(currentCpu);
    uint64_t total = 0, available = 0;
    if (!dotnet_pal_topology::physical(total, available) || total == 0) return false;
    g_totalPhysicalMemSize = static_cast<int64_t>(total > static_cast<uint64_t>(INT64_MAX) ? INT64_MAX : total);
    return true;"""

PREFIX = f'''#ifdef {MARKER}
#include "topology_adapter.h"
static bool g_palCanGetCurrentProcessor = false;
// NUMA placement is not part of the boundary: one node, no memory policy binding.
extern "C" {{ int g_highestNumaNode = 0; bool g_numaAvailable = false; }}
int GetNumaNodeNumByCpu(int) {{ return -1; }}
#endif
'''


def gc(text):
    """gcenv.unix.cpp after the kernel and support patches."""
    if MARKER in text:
        raise ValueError("gc environment is already patched")
    text = function(text, "GCToOSInterface::Initialize", INITIALIZE)
    text = once(text, "#ifdef DOTNET_PAL_KERNEL\n    CleanupCGroup();\n#else", f"#if defined({MARKER})\n#elif defined(DOTNET_PAL_KERNEL)\n    CleanupCGroup();\n#else")
    text = function(text, "GCToOSInterface::GetCurrentProcessorNumber",
                    "    uint32_t cpu = 0;\n    if (!dotnet_pal_topology::current_cpu(cpu)) { assert(!\"current CPU is unavailable\"); return 0; }\n    return cpu;")
    text = function(text, "GCToOSInterface::CanGetCurrentProcessorNumber", "    return g_palCanGetCurrentProcessor;")
    text = function(text, "GCToOSInterface::SetThreadAffinity", "    return dotnet_pal_topology::set_thread_affinity(procNo);")
    text = function(text, "GCToOSInterface::GetVirtualMemoryLimit",
                    "    uint64_t limit = dotnet_pal_topology::virtual_limit();\n    if (limit != 0 && limit <= SIZE_MAX) return static_cast<size_t>(limit);\n    return GetVirtualMemoryMaxAddress();")
    text = function(text, "GetAvailablePhysicalMemory",
                    "    uint64_t total = 0, available = 0;\n    return dotnet_pal_topology::physical(total, available) ? available : 0;")
    # Swap accounting is not exposed by the boundary; the GC treats 0 as unknown.
    text = function(text, "GetAvailablePageFile", "    return 0;")
    text = function(text, "GetLogicalProcessorCacheSizeFromOS",
                    "    size_t cacheLevel = 0;\n    size_t cacheSize = dotnet_pal_topology::cache_size();\n"
                    "#if (defined(HOST_ARM64) || defined(HOST_LOONGARCH64)) && !defined(TARGET_APPLE)\n"
                    "    // The boundary reports no cache level, so the upstream heuristic applies as a lower bound.\n"
                    "    size_t heuristic = 0;\n    GetLogicalProcessorCacheSizeFromHeuristic(&cacheLevel, &heuristic);\n    if (heuristic > cacheSize) cacheSize = heuristic;\n"
                    "#else\n    if (cacheSize == 0) GetLogicalProcessorCacheSizeFromHeuristic(&cacheLevel, &cacheSize);\n#endif\n    return cacheSize;")
    # Readers that only serve the replaced paths stay out of the PAL build.
    for name in ("bool ReadMemoryValueFromFile(const char* filename, uint64_t* val)\n{",
                 "static void GetLogicalProcessorCacheSizeFromSysConf(size_t* cacheLevel, size_t* cacheSize)\n{",
                 "static void GetLogicalProcessorCacheSizeFromSysFs(size_t* cacheLevel, size_t* cacheSize)\n{",
                 "static bool ReadMemAvailable(uint64_t* memAvailable)\n{"):
        start = text.index(name)
        end = text.index("\n}\n", start) + 3
        text = text[:start] + f"#ifndef {MARKER}\n" + text[start:end] + "#endif\n" + text[end:]
    text = once(text, "static uint64_t GetMemorySizeMultiplier(char units)\n{", f"#ifndef {MARKER}\nstatic uint64_t GetMemorySizeMultiplier(char units)\n{{")
    text = once(text, "    return 1;\n}\n\n#if !defined(__APPLE__) && !defined(__HAIKU__)\n", "    return 1;\n}\n#endif\n\n#if !defined(__APPLE__) && !defined(__HAIKU__)\n")
    if text.count("#if HAVE_PROCFS_STATM\n") != 2:
        raise ValueError("expected the statm reader and its use")
    text = text.replace("#if HAVE_PROCFS_STATM\n", f"#if HAVE_PROCFS_STATM && !defined({MARKER})\n")
    declarations = "size_t GetRestrictedPhysicalMemoryLimit();\nbool GetPhysicalMemoryUsed(size_t* val);"
    definitions = f'''{declarations}
#ifdef {MARKER}
size_t GetRestrictedPhysicalMemoryLimit()
{{
    uint64_t limit = dotnet_pal_topology::memory_limit();
    return limit > SIZE_MAX ? SIZE_MAX : static_cast<size_t>(limit);
}}
bool GetPhysicalMemoryUsed(size_t* val)
{{
    uint64_t total = 0, available = 0;
    if (val == nullptr || !dotnet_pal_topology::physical(total, available)) return false;
    uint64_t used = total - available;
    *val = used > SIZE_MAX ? SIZE_MAX : static_cast<size_t>(used);
    return true;
}}
#endif'''
    text = once(text, declarations, definitions)
    return PREFIX + text


CPU_COUNT = """    uint32_t count = 0;
    const unsigned int MAX_PROCESSOR_COUNT = 0xffff;
    uint64_t configValue;
    if (g_pRhConfig->ReadConfigValue("PROCESSOR_COUNT", &configValue, true /* decimal */) &&
        0 < configValue && configValue <= MAX_PROCESSOR_COUNT)
    {
        count = configValue;
    }
    else
    {
        count = dotnet_pal_topology::cpu_count();
    }
    _ASSERTE(count > 0);
    g_RhNumberOfProcessors = count;"""


def pal(text):
    """PalUnix.cpp after the other PAL patches."""
    text = once(text, "bool PalInit()\n{", f"bool PalInit()\n{{\n#ifdef {MARKER}\n    if (!dotnet_pal_topology::initialize()) return false;\n#endif")
    text = function(text, "InitializeCurrentProcessCpuCount", CPU_COUNT)
    text = once(text, "    InitializeCpuCGroup();\n", f"#ifndef {MARKER}\n    InitializeCpuCGroup();\n#endif\n")
    return f'#ifdef {MARKER}\n#include "topology_adapter.h"\n#endif\n' + text


TRANSFORMS = {CGROUP: cgroup, CGROUP_CPU: cgroup_cpu, NUMA: numa}
