"""Route the OS calls of minipal (the runtime's small platform helper library)
through the PAL: clock, thread id, debugger presence, entropy, log output,
mutexes, CPU count and CPU feature words. minipal is C, so the front end is the
C header native/minipal_pal_adapter.h. The rebuilt libaotminipal.a is audited
with the runtime archive.
"""
from kernel_patch import once

MARKER = "DOTNET_PAL_MINIPAL"
ROOT = "src/native/minipal/"
TIME = ROOT + "time.c"
THREAD = ROOT + "thread.h"
DEBUGGER = ROOT + "debugger.c"
RANDOM = ROOT + "random.c"
LOG = ROOT + "log.c"
MUTEX_HEADER = ROOT + "mutex.h"
MUTEX = ROOT + "mutex.c"
CPUCOUNT = ROOT + "cpucount.c"
CPUFEATURES = ROOT + "cpufeatures.c"
CMAKE = ROOT + "CMakeLists.txt"
INCLUDE = f'#ifdef {MARKER}\n#include "minipal_pal_adapter.h"\n#endif\n'


def time(text):
    text = once(text, "int64_t minipal_hires_ticks(void)\n{\n#if HAVE_CLOCK_GETTIME_NSEC_NP",
                f"int64_t minipal_hires_ticks(void)\n{{\n#ifdef {MARKER}\n    return dotnet_pal_minipal_ticks_ns();\n#elif HAVE_CLOCK_GETTIME_NSEC_NP")
    text = once(text, "int64_t minipal_lowres_ticks(void)\n{\n#if HAVE_CLOCK_GETTIME_NSEC_NP",
                f"int64_t minipal_lowres_ticks(void)\n{{\n#ifdef {MARKER}\n    return dotnet_pal_minipal_ticks_ns() / (int64_t)tccMilliSecondsToNanoSeconds;\n#elif HAVE_CLOCK_GETTIME_NSEC_NP")
    text = once(text, "#else\n    if (usecs > 10)\n    {\n        struct timespec requested;",
                f"#elif defined({MARKER})\n    if (usecs > 10 && dotnet_pal_minipal_sleep_ns((uint64_t)usecs * 1000))\n    {{\n"
                "        if (usecsSinceYield)\n        {\n            *usecsSinceYield = 0;\n        }\n        return;\n    }\n"
                "#else\n    if (usecs > 10)\n    {\n        struct timespec requested;")
    return INCLUDE + text


def thread(text):
    text = once(text, "    tid = (size_t)syscall(SYS_gettid);", f"#ifdef {MARKER}\n    tid = dotnet_pal_minipal_thread_id();\n#else\n    tid = (size_t)syscall(SYS_gettid);\n#endif")
    return once(text, "#ifndef HOST_WINDOWS\n\n#include <pthread.h>\n", "#ifndef HOST_WINDOWS\n\n#include <pthread.h>\n" + INCLUDE)


def debugger(text):
    text = once(text, "bool minipal_can_check_for_native_debugger(void)\n{\n#if defined(MINIPAL_DEBUGGER_PRESENT_CHECK)",
                f"bool minipal_can_check_for_native_debugger(void)\n{{\n#if defined({MARKER})\n    return dotnet_pal_minipal_can_check_debugger();\n#elif defined(MINIPAL_DEBUGGER_PRESENT_CHECK)")
    text = once(text, "bool minipal_is_native_debugger_present(void)\n{\n#if defined(_WIN32)",
                f"bool minipal_is_native_debugger_present(void)\n{{\n#if defined({MARKER})\n    return dotnet_pal_minipal_debugger_present();\n#elif defined(_WIN32)")
    return INCLUDE + text


def random(text):
    text = once(text, "#if HAVE_ARC4RANDOM_BUF\n    arc4random_buf(buffer, (size_t)bufferLength);\n#elif HAVE_BCRYPT_H",
                f"""#if defined({MARKER})
    if (dotnet_pal_minipal_random(buffer, bufferLength) != 0)
    {{
        // No entropy source on this port: a clock-seeded xoshiro stream, explicitly not secure.
        static struct minipal_xoshiro128pp state;
        static bool seeded;
        uint32_t word = 0;
        if (!seeded)
        {{
            minipal_xoshiro128pp_init(&state, (uint32_t)dotnet_pal_minipal_ticks_ns());
            seeded = true;
        }}
        for (int32_t i = 0; i < bufferLength; i++)
        {{
            if (i % 4 == 0)
            {{
                word = minipal_xoshiro128pp_next(&state);
            }}
            buffer[i] = (uint8_t)word;
            word >>= 8;
        }}
    }}
#elif HAVE_ARC4RANDOM_BUF
    arc4random_buf(buffer, (size_t)bufferLength);
#elif HAVE_BCRYPT_H""")
    text = once(text, "#ifdef __EMSCRIPTEN__\n    extern int32_t mono_wasm_browser_entropy",
                f"#if defined({MARKER})\n    return dotnet_pal_minipal_random(buffer, bufferLength);\n#elif defined(__EMSCRIPTEN__)\n    extern int32_t mono_wasm_browser_entropy")
    return INCLUDE + '#include "xoshiro128pp.h"\n' + text


LOG_IMPLEMENTATION = f"""#elif defined({MARKER})
#include <stdio.h>
#define MINIPAL_LOG_BUF_SIZE 1024
// Every level reaches the port's single diagnostic channel; there is no stdout,
// and a formatted line longer than the stack buffer is truncated rather than
// allocated (no heap in the diagnostic path).
int minipal_log_write(minipal_log_flags flags, const char* msg)
{{
    (void)flags;
    assert(msg != NULL);
    return (int)dotnet_pal_minipal_write(msg, strlen(msg));
}}
int minipal_log_vprint(minipal_log_flags flags, const char* fmt, va_list args)
{{
    char stack_buffer[MINIPAL_LOG_BUF_SIZE];
    int len = vsnprintf(stack_buffer, sizeof(stack_buffer), fmt, args);
    if (len < 0)
        return 0;
    size_t count = (size_t)len < sizeof(stack_buffer) ? (size_t)len : sizeof(stack_buffer) - 1;
    (void)flags;
    return (int)dotnet_pal_minipal_write(stack_buffer, count);
}}
int minipal_log_print(minipal_log_flags flags, const char* fmt, ... )
{{
    va_list args;
    va_start(args, fmt);
    int bytes_written = minipal_log_vprint(flags, fmt, args);
    va_end(args);
    return bytes_written;
}}
void minipal_log_flush(minipal_log_flags flags) {{ (void)flags; }}
void minipal_log_flush_all(void) {{ }}
void minipal_log_sync(minipal_log_flags flags) {{ (void)flags; }}
void minipal_log_sync_all(void) {{ }}
#else"""


def log(text):
    text = once(text, "#else\n#include <errno.h>\n#include <stdio.h>\n\n#define MINIPAL_LOG_MAX_PAYLOAD 32767",
                LOG_IMPLEMENTATION + "\n#include <errno.h>\n#include <stdio.h>\n\n#define MINIPAL_LOG_MAX_PAYLOAD 32767")
    return INCLUDE + text


def mutex_header(text):
    return once(text, "#else // !HOST_WINDOWS\n#include <pthread.h>\ntypedef pthread_mutex_t MINIPAL_MUTEX_IMPL;",
                f"#elif defined({MARKER})\ntypedef void* MINIPAL_MUTEX_IMPL; // a PAL mutex handle\n#else // !HOST_WINDOWS\n#include <pthread.h>\ntypedef pthread_mutex_t MINIPAL_MUTEX_IMPL;")


def mutex(text):
    edits = {
        "#ifdef HOST_WINDOWS\n    InitializeCriticalSection(&mtx->_impl);\n    return true;\n#else":
            f"#if defined({MARKER})\n    return dotnet_pal_minipal_kernel()->mutex_create(1, &mtx->_impl) == DOTNET_PAL_OK;\n#elif defined(HOST_WINDOWS)\n    InitializeCriticalSection(&mtx->_impl);\n    return true;\n#else",
        "#ifdef HOST_WINDOWS\n    DeleteCriticalSection(&mtx->_impl);\n#else":
            f"#if defined({MARKER})\n    if (dotnet_pal_minipal_kernel()->mutex_destroy(mtx->_impl) != DOTNET_PAL_OK) __builtin_trap();\n#elif defined(HOST_WINDOWS)\n    DeleteCriticalSection(&mtx->_impl);\n#else",
        "#ifdef HOST_WINDOWS\n    EnterCriticalSection(&mtx->_impl);\n#else":
            f"#if defined({MARKER})\n    if (dotnet_pal_minipal_kernel()->mutex_lock(mtx->_impl) != DOTNET_PAL_OK) __builtin_trap();\n#elif defined(HOST_WINDOWS)\n    EnterCriticalSection(&mtx->_impl);\n#else",
        "#ifdef HOST_WINDOWS\n    LeaveCriticalSection(&mtx->_impl);\n#else":
            f"#if defined({MARKER})\n    if (dotnet_pal_minipal_kernel()->mutex_unlock(mtx->_impl) != DOTNET_PAL_OK) __builtin_trap();\n#elif defined(HOST_WINDOWS)\n    LeaveCriticalSection(&mtx->_impl);\n#else",
    }
    for old, new in edits.items():
        text = once(text, old, new)
    return INCLUDE + text


def cpucount(text):
    text = once(text, "#if defined(__linux__)\n    FILE* f = fopen(\"/sys/devices/system/cpu/possible\", \"r\");",
                f"#if defined({MARKER})\n    return (int)dotnet_pal_minipal_cpu_max();\n#elif defined(__linux__)\n    FILE* f = fopen(\"/sys/devices/system/cpu/possible\", \"r\");")
    text = once(text, "#endif\n\n    return (int)sysconf(_SC_NPROCESSORS_CONF);\n", f"#endif\n#ifndef {MARKER}\n    return (int)sysconf(_SC_NPROCESSORS_CONF);\n#endif\n")
    return INCLUDE + text


def cpufeatures(text):
    text = once(text, "#if HAVE_AUXV_HWCAP_H\n    unsigned long hwCap = getauxval(AT_HWCAP);",
                f"#if defined({MARKER}) || HAVE_AUXV_HWCAP_H\n#ifdef {MARKER}\n    uint64_t palHwCap = 0, palHwCap2 = 0;\n    dotnet_pal_minipal_cpu_features(&palHwCap, &palHwCap2);\n"
                "    unsigned long hwCap = (unsigned long)palHwCap;\n#else\n    unsigned long hwCap = getauxval(AT_HWCAP);\n#endif")
    text = once(text, "    unsigned long hwCap2 = getauxval(AT_HWCAP2);",
                f"#ifdef {MARKER}\n    unsigned long hwCap2 = (unsigned long)palHwCap2;\n#else\n    unsigned long hwCap2 = getauxval(AT_HWCAP2);\n#endif")
    return INCLUDE + text


def cmake(text):
    if MARKER in text:
        raise ValueError("minipal CMake is already patched")
    return once(text, "add_library(aotminipal STATIC ${SOURCES})",
                f"if(DOTNET_PAL_ROOT)\n  add_definitions(-D{MARKER}=1)\n  include_directories(\"${{DOTNET_PAL_ROOT}}/include\" \"${{DOTNET_PAL_ROOT}}/native\")\nendif()\n\nadd_library(aotminipal STATIC ${{SOURCES}})")


FILES = (TIME, THREAD, DEBUGGER, RANDOM, LOG, MUTEX_HEADER, MUTEX, CPUCOUNT, CPUFEATURES, CMAKE)
TRANSFORMS = {TIME: time, THREAD: thread, DEBUGGER: debugger, RANDOM: random, LOG: log, MUTEX_HEADER: mutex_header,
              MUTEX: mutex, CPUCOUNT: cpucount, CPUFEATURES: cpufeatures, CMAKE: cmake}
