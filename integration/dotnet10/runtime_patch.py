"""Source adapters for neutral runtime services; no symbol renaming/wrapping."""
from kernel_patch import once
import re

def function(text, name, body):
    pattern = re.compile(rf"(^[^\n]*\b{re.escape(name)}\([^)]*\)\n\{{\n)(.*?)(^\}})", re.M | re.S)
    if len(list(pattern.finditer(text))) != 1: raise ValueError(f"expected one definition: {name}")
    return pattern.sub(lambda m: m[1] + "#ifdef DOTNET_PAL_RUNTIME\n" + body + "\n#else\n" + m[2] + "#endif\n" + m[3], text)


def pal(text):
    text = once(text, "bool PalInit()\n{", "bool PalInit()\n{\n#ifdef DOTNET_PAL_RUNTIME\n    if (!dotnet_pal_runtime::initialize()) return false;\n#endif")
    bodies = {
        "PalGetEnvironmentVariable": "    return dotnet_pal_runtime::environment(name, buffer, size);",
        "PalGetCurrentProcessId": "    return static_cast<uint32_t>(dotnet_pal_runtime::identity(false));",
        "PalGetCurrentOSThreadId": "    return dotnet_pal_runtime::identity(true);",
        "PalLoadLibrary": "    return dotnet_pal_runtime::open(moduleName);",
        "PalGetProcAddress": "    return dotnet_pal_runtime::symbol(module, functionName);",
        "PalGetModuleHandleFromPointer": "    return dotnet_pal_runtime::module_base(pointer);",
        "PalGetModuleFileName": "    return dotnet_pal_runtime::module_name(moduleBase, pModuleNameOut);",
        "PalVirtualAlloc": "    return dotnet_pal_runtime::allocate(size, protect);",
        "PalVirtualFree": "    dotnet_pal_runtime::release(pAddress, size);",
        "PalVirtualProtect": "    return dotnet_pal_runtime::protect(pAddress, size, protect);",
        "PalMarkThunksAsValidCallTargets": "    if (thunkBlocksPerMapping <= 0 || static_cast<size_t>(thunkBlocksPerMapping) > SIZE_MAX / OS_PAGE_SIZE) return UInt32_FALSE;\n    size_t bytes = static_cast<size_t>(thunkBlocksPerMapping) * OS_PAGE_SIZE;\n    return dotnet_pal_runtime::protect(static_cast<uint8_t*>(virtualAddress) + bytes, bytes, PAGE_READWRITE);",
    }
    for name, body in bodies.items():
        text = function(text, name, body)
    # The old translation helper becomes unused once all its callers are routed.
    import re
    pattern = re.compile(r"^static int W32toUnixAccessControl\([^\n]*\)\n\{\n.*?^\}", re.M|re.S)
    if len(list(pattern.finditer(text))) != 1: raise ValueError("missing protection helper")
    text = pattern.sub(lambda m: "#ifndef DOTNET_PAL_RUNTIME\n" + m[0] + "\n#endif", text)
    old = "    g_RhPageSize = (uint32_t)sysconf(_SC_PAGE_SIZE);"
    text = once(text, old, "#ifdef DOTNET_PAL_RUNTIME\n    g_RhPageSize = static_cast<uint32_t>(dotnet_pal_runtime::require()->vm.page_size());\n#else\n" + old + "\n#endif")
    return '#ifdef DOTNET_PAL_RUNTIME\n#include "runtime_adapter.h"\n#endif\n' + text
