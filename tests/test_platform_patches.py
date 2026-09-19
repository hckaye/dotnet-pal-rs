from pathlib import Path
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'integration/dotnet10'))
import topology_patch, process_patch, image_patch, minipal_patch  # noqa: E402


class TopologyPatchTests(unittest.TestCase):
    def test_guarded_function_uses_requested_marker(self):
        text = 'bool Example(int n)\n{\n    return old(n);\n}\n'
        result = topology_patch.function(text, 'Example', '    return replacement(n);', 'DOTNET_PAL_X')
        self.assertIn('#ifdef DOTNET_PAL_X\n    return replacement(n);\n#else\n    return old(n);\n#endif\n}', result)
        with self.assertRaises(ValueError): topology_patch.function(text + text, 'Example', 'x')

    def test_compiled_out_files_keep_the_upstream_reader_for_other_builds(self):
        result = topology_patch.cgroup('int reader() { return open("/proc"); }\n')
        self.assertTrue(result.startswith('#ifdef DOTNET_PAL_TOPOLOGY\n'))
        self.assertIn('#else\nint reader() { return open("/proc"); }\n', result)
        self.assertTrue(result.rstrip().endswith('#endif // DOTNET_PAL_TOPOLOGY'))
        with self.assertRaises(ValueError): topology_patch.cgroup(result)

    def test_gc_environment_requires_every_pinned_anchor(self):
        with self.assertRaises(ValueError): topology_patch.gc('bool GCToOSInterface::Initialize()\n{\n    x;\n}\n')
        with self.assertRaises(ValueError): topology_patch.gc('#ifdef DOTNET_PAL_TOPOLOGY')

    def test_thread_identity_routes_through_runtime_group(self):
        old = topology_patch.structs.__code__.co_consts
        text = ("class EEThreadId\n{\n    pthread_t m_id;\n    // Indicates whether the m_id is valid or not. pthread_t doesn't have any\n"
                "    // portable \"invalid\" value.\n    bool m_isValid;\n\npublic:\n    bool IsCurrentThread()\n    {\n"
                "        return m_isValid && pthread_equal(m_id, pthread_self());\n    }\n\n    void SetToCurrentThread()\n    {\n"
                "        m_id = pthread_self();\n        m_isValid = true;\n    }\n")
        result = topology_patch.structs(text)
        self.assertIn('dotnet_pal_runtime::identity(true)', result)
        self.assertIn('#else\nclass EEThreadId\n{\n    pthread_t m_id;', result)
        self.assertIsNotNone(old)

    def test_pal_cpu_count_and_cgroup_init(self):
        text = ('bool PalInit()\n{\n    InitializeCpuCGroup();\n    x;\n}\n'
                'void InitializeCurrentProcessCpuCount()\n{\n    old();\n}\n')
        result = topology_patch.pal(text)
        self.assertIn('dotnet_pal_topology::initialize()', result)
        self.assertIn('#ifndef DOTNET_PAL_TOPOLOGY\n    InitializeCpuCGroup();\n#endif', result)
        self.assertIn('count = dotnet_pal_topology::cpu_count();', result)


class ProcessPatchTests(unittest.TestCase):
    HEADER = "CreateCrashDump(\n    const char* argv[],\n    char* errorMessageBuffer,\n    int cbErrorMessageBuffer)\n{\n"

    def test_launch_is_routed_and_path_lookup_uses_runtime_group(self):
        text = ('static bool\n' + self.HEADER + '    int pipe_descs[4];\n    if (fork()) { }\n}\n'
                '    RhConfig::Environment::TryGetBooleanValue("DbgEnableMiniDump", &enabled);\n    if (enabled)\n    {\n'
                + process_patch.OLD_PATH + '\n')
        result = process_patch.dump(text)
        self.assertIn('dotnet_pal_process::crash_dump(argv, argc', result)
        self.assertIn('#else\n    int pipe_descs[4];\n    if (fork()) { }\n#endif\n}', result)
        self.assertIn('dotnet_pal_runtime::module_name((void*)&PalCreateDumpInitialize', result)
        self.assertIn('dotnet_pal_process::can_dump()', result)
        with self.assertRaises(ValueError): process_patch.dump(result)

    def test_missing_launch_definition_is_rejected(self):
        with self.assertRaises(ValueError): process_patch.dump('nothing here')


class ImagePatchTests(unittest.TestCase):
    def test_unwind_lookup_replaces_program_header_walk(self):
        text = ('#if defined(_LIBUNWIND_USE_DL_ITERATE_PHDR)\n\n// The ElfW() macro\n'
                'namespace libunwind {\n\n/// Used by findUnwindSections()\n'
                '#elif defined(_LIBUNWIND_USE_DL_ITERATE_PHDR)\n  // Use DLFO_STRUCT_HAS_EH_DBASE to determine the existence of\n'
                '  int found = dl_iterate_phdr(findUnwindSectionsByPhdr, &cb_data);\n  return static_cast<bool>(found);\n#endif\n'
                '#if _LIBUNWIND_USE_DLADDR\n  Dl_info dyldInfo;\n')
        result = image_patch.address_space(text)
        self.assertIn('dotnet_pal_image::unwind(targetAddr, pal)', result)
        self.assertIn('#endif // DOTNET_PAL_IMAGE\n#endif\n', result)
        self.assertIn('#if defined(_LIBUNWIND_USE_DL_ITERATE_PHDR) && !defined(DOTNET_PAL_IMAGE)\n', result)
        self.assertIn('#if _LIBUNWIND_USE_DLADDR && !defined(DOTNET_PAL_IMAGE)', result)
        with self.assertRaises(ValueError): image_patch.address_space(result)

    def test_readability_probe_is_routed(self):
        text = ('  const auto sigsetAddr = reinterpret_cast<sigset_t *>(addr);\n  syscall(...);\n'
                '  const auto readable = errno != EFAULT;\n  errno = saveErrno;\n  return readable;\n}\n')
        result = image_patch.cursor(text)
        self.assertIn('#ifdef DOTNET_PAL_IMAGE\n  return addr != 0 && dotnet_pal_image::readable(addr, NSIG / 8);\n#else\n', result)
        self.assertIn('  return readable;\n#endif\n}', result)
        with self.assertRaises(ValueError): image_patch.cursor('')

    def test_logging_macros_are_redefined_only_for_cxx(self):
        result = image_patch.config('base')
        self.assertIn('#if defined(DOTNET_PAL_IMAGE) && defined(__cplusplus)', result)
        self.assertIn('#undef _LIBUNWIND_ABORT', result)

    def test_build_id_lookup_in_pal(self):
        text = ('bool PalInit()\n{\n    x;\n}\n#if TARGET_LINUX\n\nstruct PalGetPDBInfoPhdrCallbackData\n'
                'void PalGetPDBInfo(HANDLE hOsHandle, GUID * pGuidSignature)\n{\n    old;\n}\n')
        result = image_patch.pal(text)
        self.assertIn('dotnet_pal_image::build_id(reinterpret_cast<uintptr_t>(hOsHandle), data, length)', result)
        self.assertIn('#if TARGET_LINUX && !defined(DOTNET_PAL_IMAGE)', result)


class MinipalPatchTests(unittest.TestCase):
    def test_clock_and_delay_keep_upstream_paths(self):
        text = ('int64_t minipal_hires_ticks(void)\n{\n#if HAVE_CLOCK_GETTIME_NSEC_NP\n'
                'int64_t minipal_lowres_ticks(void)\n{\n#if HAVE_CLOCK_GETTIME_NSEC_NP\n'
                '#else\n    if (usecs > 10)\n    {\n        struct timespec requested;\n')
        result = minipal_patch.time(text)
        self.assertIn('#ifdef DOTNET_PAL_MINIPAL\n    return dotnet_pal_minipal_ticks_ns();\n#elif HAVE_CLOCK_GETTIME_NSEC_NP', result)
        self.assertIn('dotnet_pal_minipal_sleep_ns((uint64_t)usecs * 1000)', result)
        self.assertIn('#else\n    if (usecs > 10)\n    {\n        struct timespec requested;', result)

    def test_thread_id_and_mutex_and_cpu_paths(self):
        thread = minipal_patch.thread('#ifndef HOST_WINDOWS\n\n#include <pthread.h>\n    tid = (size_t)syscall(SYS_gettid);\n')
        self.assertIn('tid = dotnet_pal_minipal_thread_id();', thread)
        mutex = minipal_patch.mutex('#ifdef HOST_WINDOWS\n    InitializeCriticalSection(&mtx->_impl);\n    return true;\n#else\n'
                                    '#ifdef HOST_WINDOWS\n    DeleteCriticalSection(&mtx->_impl);\n#else\n'
                                    '#ifdef HOST_WINDOWS\n    EnterCriticalSection(&mtx->_impl);\n#else\n'
                                    '#ifdef HOST_WINDOWS\n    LeaveCriticalSection(&mtx->_impl);\n#else\n')
        self.assertEqual(mutex.count('dotnet_pal_minipal_kernel()'), 4)
        cpu = minipal_patch.cpucount('#if defined(__linux__)\n    FILE* f = fopen("/sys/devices/system/cpu/possible", "r");\n#endif\n\n    return (int)sysconf(_SC_NPROCESSORS_CONF);\n')
        self.assertIn('return (int)dotnet_pal_minipal_cpu_max();', cpu)
        self.assertIn('#ifndef DOTNET_PAL_MINIPAL\n    return (int)sysconf(_SC_NPROCESSORS_CONF);\n#endif', cpu)
        features = minipal_patch.cpufeatures('#if HAVE_AUXV_HWCAP_H\n    unsigned long hwCap = getauxval(AT_HWCAP);\n    unsigned long hwCap2 = getauxval(AT_HWCAP2);\n')
        self.assertIn('dotnet_pal_minipal_cpu_features(&palHwCap, &palHwCap2);', features)

    def test_log_has_no_heap_and_cmake_defines_marker_once(self):
        log = minipal_patch.log('#else\n#include <errno.h>\n#include <stdio.h>\n\n#define MINIPAL_LOG_MAX_PAYLOAD 32767\n')
        self.assertIn('dotnet_pal_minipal_write(stack_buffer, count)', log)
        self.assertNotIn('malloc(', log)
        cmake = minipal_patch.cmake('add_library(aotminipal STATIC ${SOURCES})\n')
        self.assertIn('add_definitions(-DDOTNET_PAL_MINIPAL=1)', cmake)
        with self.assertRaises(ValueError): minipal_patch.cmake(cmake)


if __name__ == '__main__':
    unittest.main()
