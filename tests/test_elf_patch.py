import sys, unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'integration/dotnet10'))
import elf_patch
class ElfPatchTests(unittest.TestCase):
    def test_source_guards_preserved(self):
        s='void f(){dl_iterate_phdr(callback, data);}'
        result=elf_patch.pal(s)
        self.assertIn('DOTNET_PAL_ELF_ITERATE(callback, data)', result)
        self.assertIn('#else', result)
        for bad in ('',s+s,result):
            with self.assertRaises(ValueError):elf_patch.pal(bad)
    def test_direct_dlsym_optimization_not_mislabeled_as_routed(self):
        text=(Path(__file__).resolve().parents[1]/'integration/dotnet10/elf_patch.py').read_text()
        self.assertIn('!defined(DOTNET_PAL_ELF_METADATA)',text)
    def test_sparse_checkout_keeps_all_inputs(self):
        workflow=(Path(__file__).resolve().parents[1]/'.github/workflows/ci.yml').read_text()
        for f in elf_patch.FILES:self.assertIn('/'+f,workflow)
