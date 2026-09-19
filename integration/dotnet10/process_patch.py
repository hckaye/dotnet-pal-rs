"""Route the crash-dump utility launch through the PAL process group.

The runtime keeps its argument building, configuration parsing and the module
path lookup (through the runtime group); only spawning the utility and waiting
for it become a port service. A port without that service refuses startup when
DOTNET_DbgEnableMiniDump asks for dumps, instead of silently producing none.
"""
import re
from kernel_patch import once

MARKER = "DOTNET_PAL_PROCESS"
DUMP = "src/coreclr/nativeaot/Runtime/unix/PalCreateDump.cpp"
PAL = "src/coreclr/nativeaot/Runtime/unix/PalUnix.cpp"
FILES = ()

LAUNCH = """    size_t argc = 0;
    while (argv[argc] != nullptr) argc++;
    return dotnet_pal_process::crash_dump(argv, argc, errorMessageBuffer, errorMessageBuffer ? static_cast<size_t>(cbErrorMessageBuffer) : 0);"""

OLD_PATH = """        Dl_info info;
        if (dladdr((void*)&PalCreateDumpInitialize, &info) == 0)
        {
            return false;
        }
        const char* DumpGeneratorName = "createdump";
        int programLen = strlen(info.dli_fname) + strlen(DumpGeneratorName) + 1;
        char* program = (char*)DOTNET_PAL_DUMP_ALLOC(programLen);
        if (program == nullptr)
        {
            return false;
        }
        strncpy(program, info.dli_fname, programLen);"""

NEW_PATH = """        const char* moduleName = nullptr;
        int32_t moduleNameLength = dotnet_pal_runtime::module_name((void*)&PalCreateDumpInitialize, &moduleName);
        if (moduleName == nullptr || moduleNameLength < 0)
        {
            return false;
        }
        const char* DumpGeneratorName = "createdump";
        int programLen = moduleNameLength + strlen(DumpGeneratorName) + 1;
        char* program = (char*)DOTNET_PAL_DUMP_ALLOC(programLen);
        if (program == nullptr)
        {
            return false;
        }
        memcpy(program, moduleName, moduleNameLength);
        program[moduleNameLength] = '\\0';"""


def dump(text):
    """PalCreateDump.cpp after the support patch renamed its allocator calls."""
    if MARKER in text:
        raise ValueError("dump launch is already patched")
    header = "CreateCrashDump(\n    const char* argv[],\n    char* errorMessageBuffer,\n    int cbErrorMessageBuffer)\n{\n"
    regex = re.compile(re.escape(header) + r"(.*?)(^\})", re.M | re.S)
    if len(list(regex.finditer(text))) != 1:
        raise ValueError("expected one pinned CreateCrashDump definition")
    text = regex.sub(lambda m: header + f"#ifdef {MARKER}\n{LAUNCH}\n#else\n" + m[1].rstrip() + "\n#endif\n" + m[2], text)
    text = once(text, OLD_PATH, f"#ifdef {MARKER}\n{NEW_PATH}\n#else\n{OLD_PATH}\n#endif")
    text = once(text, "    RhConfig::Environment::TryGetBooleanValue(\"DbgEnableMiniDump\", &enabled);\n    if (enabled)\n    {\n",
                "    RhConfig::Environment::TryGetBooleanValue(\"DbgEnableMiniDump\", &enabled);\n    if (enabled)\n    {\n"
                f"#ifdef {MARKER}\n        if (!dotnet_pal_process::can_dump())\n        {{\n"
                "            dotnet_pal_support::fatal_message(\"DOTNET_DbgEnableMiniDump is set, but this port has no crash dump utility launcher\\n\");\n"
                "            return false;\n        }\n#endif\n")
    return f'#ifdef {MARKER}\n#include "process_adapter.h"\n#include "runtime_adapter.h"\n#include <cstring>\n#endif\n' + text


def pal(text):
    """PalUnix.cpp: negotiate the optional group once at startup."""
    text = once(text, "bool PalInit()\n{", f"bool PalInit()\n{{\n#ifdef {MARKER}\n    if (!dotnet_pal_process::initialize()) return false;\n#endif")
    return f'#ifdef {MARKER}\n#include "process_adapter.h"\n#endif\n' + text


TRANSFORMS = {}
