import importlib.util
from pathlib import Path
import sys
import unittest

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'integration/dotnet10'))
import support_patch as patch


class SupportPatchTests(unittest.TestCase):
    def test_guarded_definition_keeps_original(self):
        text='bool Example(int n)\n{\n    return old(n);\n}\n'
        result=patch.function(text,'Example','    return replacement(n);')
        self.assertIn('#ifdef DOTNET_PAL_SUPPORT\n    return replacement(n);\n#else\n    return old(n);',result)

    def test_missing_or_duplicate_definition_rejected(self):
        text='bool Example(int n)\n{\n    return old(n);\n}\n'
        for value in ('',text+text):
            with self.assertRaises(ValueError):patch.function(value,'Example','return true;')

    def test_dump_redirection_requires_audited_counts(self):
        text=('x=malloc(2);\n'*3)+('free(x);\n'*8)+'getpid();\n'
        result=patch.dump(text)
        self.assertEqual(result.count('DOTNET_PAL_DUMP_ALLOC('),3)
        self.assertEqual(result.count('DOTNET_PAL_DUMP_FREE('),8)
        self.assertIn('dotnet_pal_context::process_id_async()',result)
        with self.assertRaises(ValueError):patch.dump(text+'free(x);')
        with self.assertRaises(ValueError):patch.dump(result)

    def test_rwlock_keeps_non_nativeaot_path(self):
        text='#define __RWMUTEX_HPP__\n#if defined(_LIBUNWIND_HAS_NO_THREADS)\nold\n#endif'
        result=patch.rw(text)
        self.assertIn('public dotnet_pal_support::ReadWriteLock',result)
        self.assertIn('#elif defined(_LIBUNWIND_HAS_NO_THREADS)',result)
        with self.assertRaises(ValueError):patch.rw('')

    def test_cursor_pairs_allocations_and_release(self):
        text='  static pint_t findFDE(pint_t mh, pint_t pc);\n    entry *newBuffer = (entry *)malloc(newSize * sizeof(entry));\nfree(_buffer);\n'
        result=patch.cursor(text)
        self.assertIn('dotnet_pal_support::allocate',result)
        self.assertIn('dotnet_pal_support::release',result)
        self.assertEqual(result.count('#ifdef DOTNET_PAL_SUPPORT'),3)
        self.assertIn('if (newBuffer == nullptr)',result)
        self.assertIn('oldSize > SIZE_MAX / (4 * sizeof(entry))',result)
        with self.assertRaises(ValueError):patch.cursor(text.replace('free(_buffer);',''))

    def test_signal_diagnostic_does_not_negotiate(self):
        text=(ROOT/'native/support_adapter.h').read_text()
        body=text.split('inline void fatal_message(',1)[1].split('inline uint64_t filetime()',1)[0]
        self.assertNotIn('dotnet_pal_get_api(',body)
        self.assertNotIn('initialize()',body)
        self.assertIn('installed.load',body)

    def test_capability_is_optional_and_append_only(self):
        text=(ROOT/'include/dotnet_pal.h').read_text()
        self.assertIn('dotnet_pal_context_ops context;\n    dotnet_pal_support_ops support;',text)
        self.assertIn('host-support = ["host"]',(ROOT/'Cargo.toml').read_text())
        text=(ROOT/'src/lib.rs').read_text()
        # The support group is negotiated like every other group of the port.
        self.assertIn('support::negotiate::<P>()',text)
        self.assertIn('| support_caps',text)


if __name__=='__main__':unittest.main()
