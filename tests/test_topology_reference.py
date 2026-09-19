"""Deterministic cache-summary regression for the independent C host provider."""
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


@unittest.skipUnless(platform.system() == 'Linux', 'the reference provider is Linux-only')
class HostTopologyReferenceTests(unittest.TestCase):
    def test_summary_uses_deepest_known_level_and_propagates_failure(self):
        cc = shutil.which('clang') or shutil.which('cc')
        self.assertIsNotNone(cc, 'a C compiler is required')
        # Compile the actual provider, substituting only its OS observations.
        # No sysfs entries exist in this deterministic fixture.
        fixture = r"""
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <stdio.h>
#include <unistd.h>
extern int pal_topology_fault;
extern const dotnet_pal_host_topology *dotnet_pal_host_topology_v2(void);
static long levels[4];
long pal_test_sysconf(int name) {
    const int names[] = {_SC_LEVEL1_DCACHE_SIZE, _SC_LEVEL2_CACHE_SIZE,
                         _SC_LEVEL3_CACHE_SIZE, _SC_LEVEL4_CACHE_SIZE};
    for (int i = 0; i < 4; ++i) if (name == names[i]) return levels[i];
    return -1;
}
FILE *pal_test_fopen(const char *path, const char *mode) {
    (void)path; (void)mode; return NULL;
}
int main(void) {
    const long cases[][4] = {
        {32768, 1048576, 33554432, 0},
        {32768, 0, 67108864, 0},
        {0, 0, 0, 0},
        {32768, 1048576, 33554432, 67108864},
        {32768, 1048576, 524288, 0}
    };
    const dotnet_pal_topology_ops *ops = &dotnet_pal_host_topology_v2()->ops;
    for (size_t c = 0; c < sizeof cases / sizeof cases[0]; ++c) {
        size_t expected = 0, actual = 123;
        for (int i = 0; i < 4; ++i) {
            levels[i] = cases[c][i];
            if (levels[i] > 0) expected = (size_t)levels[i];
        }
        assert(ops->cache_size(&actual) == DOTNET_PAL_OK);
        assert(actual == expected);
    }
    pal_topology_fault = 2;
    size_t actual = 123;
    assert(ops->cache_size(&actual) == DOTNET_PAL_BUFFER_TOO_SMALL && actual == 0);
}
"""
        with tempfile.TemporaryDirectory() as tmp:
            d = Path(tmp)
            (d / 'fixture.c').write_text(fixture)
            def run(*args):
                subprocess.run(args, check=True, capture_output=True, text=True, timeout=30)
            flags = ('-std=c11', '-O2', '-Wall', '-Wextra', '-Werror', '-I' + str(ROOT / 'include'))
            run(cc, *flags, '-Dsysconf=pal_test_sysconf', '-Dfopen=pal_test_fopen',
                '-c', str(ROOT / 'tests/topology_host.c'), '-o', str(d / 'provider.o'))
            run(cc, *flags, str(d / 'fixture.c'), str(d / 'provider.o'), '-o', str(d / 'test'))
            run(str(d / 'test'))
