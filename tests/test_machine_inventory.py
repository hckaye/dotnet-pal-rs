import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
import audit_machine_boundary as audit

class MachineInventoryTests(unittest.TestCase):
    def good(self):
        return [(f'archive.a[{name}.cpp.o]','dotnet_pal_get_api','U') for name in audit.MEMBERS]
    def test_members_and_entrypoints_required(self):
        self.assertTrue(audit.assess(self.good())['machine_boundary_pass'])
        self.assertFalse(audit.assess([])['machine_boundary_pass'])
        self.assertFalse(audit.assess(self.good()[:-1])['machine_boundary_pass'])
        self.assertFalse(audit.assess([(a,b,'w') for a,b,_ in self.good()])['machine_boundary_pass'])
    def test_strong_and_weak_bypasses_rejected(self):
        for kind in ('U','w','v'):
            for symbol in audit.FORBIDDEN:
                with self.subTest(kind=kind,symbol=symbol):
                    rows=self.good()+[('archive.a[gcenv.unix.cpp.o]',symbol+'@GLIBC_2.2.5',kind)]
                    self.assertFalse(audit.assess(rows)['machine_boundary_pass'])
    def test_backend_reference_is_not_attributed_to_runtime_member(self):
        rows=self.good()+[('pal.a[provider.o]','sysconf','U')]
        self.assertTrue(audit.assess(rows)['machine_boundary_pass'])
    def test_llvm_owner_spelling_supported(self):
        rows=[(f'archive.a({name}.cpp.obj)','dotnet_pal_get_api','U') for name in audit.MEMBERS]
        self.assertTrue(audit.assess(rows)['machine_boundary_pass'])
if __name__=='__main__':unittest.main()
