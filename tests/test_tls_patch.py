"""The x64 assembly must stop calling the TLS resolver in static-TLS builds."""
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'integration/dotnet10'))
import tls_patch

# The pinned macro body, also exercised against the full upstream file by the
# source-patch applicability step. Keep both the Apple and ELF default paths.
MACRO = r'''
.macro INLINE_GET_TLS_VAR Var
       .att_syntax
#if defined(__APPLE__)
        movq    _\Var@TLVP(%rip), %rdi
        callq   *(%rdi)
#else
        .byte 0x66  // data16 prefix - padding to have space for linker relaxations
        leaq    \Var@TLSGD(%rip), %rdi
        .byte 0x66
        .byte 0x66
        .byte 0x48
        callq   __tls_get_addr@PLT
#endif
       .intel_syntax noprefix
.endm
'''


class StaticTlsTests(unittest.TestCase):
    def test_original_path_is_retained_and_static_path_has_no_call(self):
        text = tls_patch.amd64(MACRO)
        self.assertIn('#elif defined(DOTNET_PAL_STATIC_TLS)', text)
        body = text.split('#elif defined(DOTNET_PAL_STATIC_TLS)', 1)[1].split('#else', 1)[0]
        self.assertIn(r'\Var@GOTTPOFF(%rip)', body)
        self.assertIn('addq    %fs:0, %rax', body)
        self.assertNotIn('callq', body)
        self.assertIn('callq   __tls_get_addr@PLT', text)

    def test_changed_duplicate_or_already_patched_source_is_refused(self):
        for text in ('', MACRO + MACRO, tls_patch.amd64(MACRO)):
            with self.assertRaises(ValueError):
                tls_patch.amd64(text)

    @unittest.skipUnless(platform.system() == 'Linux' and platform.machine() in ('x86_64', 'amd64'),
                         'the assembly execution test is x64 Linux only')
    def test_compiled_initial_exec_access_is_per_thread(self):
        cc = shutil.which('clang')
        cxx = shutil.which('clang++')
        nm = shutil.which('nm')
        self.assertTrue(cc and cxx and nm, 'clang, clang++ and nm are required')
        assembly = tls_patch.amd64(MACRO) + '''
.text
.intel_syntax noprefix
.globl read_slot_address
.type read_slot_address, @function
read_slot_address:
    sub rsp, 8
    INLINE_GET_TLS_VAR slot
    add rsp, 8
    ret
.size read_slot_address, . - read_slot_address
.section .note.GNU-stack,"",@progbits
'''
        cpp = '''
#include <cassert>
#include <thread>
#include <vector>
extern "C" {
thread_local unsigned long slot = 7;
unsigned long *read_slot_address();
}
int main() {
    assert(read_slot_address() == &slot && slot == 7);
    slot = 99;
    std::vector<std::thread> threads;
    for (unsigned long i = 0; i != 4; ++i) threads.emplace_back([i] {
        assert(slot == 7);
        slot = 1000 + i;
        for (int round = 0; round != 1000; ++round) {
            assert(read_slot_address() == &slot && *read_slot_address() == 1000 + i);
            std::this_thread::yield();
        }
    });
    for (auto &thread : threads) thread.join();
    assert(slot == 99 && *read_slot_address() == 99);
}
'''
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / 'access.S').write_text(assembly)
            (d / 'main.cpp').write_text(cpp)
            def run(*args):
                return subprocess.run(args, check=True, text=True, capture_output=True, timeout=30)
            # The default source path still contains the dynamic resolver call.
            run(cc, '-c', '-fPIC', str(d / 'access.S'), '-o', str(d / 'dynamic.o'))
            self.assertIn('__tls_get_addr', run(nm, '-u', str(d / 'dynamic.o')).stdout)
            run(cc, '-c', '-fPIC', '-DDOTNET_PAL_STATIC_TLS=1', str(d / 'access.S'), '-o', str(d / 'static.o'))
            run(cxx, '-std=c++17', '-O2', '-fPIC', '-ftls-model=initial-exec', '-c', str(d / 'main.cpp'), '-o', str(d / 'main.o'))
            for obj in ('static.o', 'main.o'):
                self.assertNotIn('__tls_get_addr', run(nm, '-u', str(d / obj)).stdout)
            run(cxx, '-pthread', str(d / 'main.o'), str(d / 'static.o'), '-o', str(d / 'test'))
            run(str(d / 'test'))


if __name__ == '__main__':
    unittest.main()
