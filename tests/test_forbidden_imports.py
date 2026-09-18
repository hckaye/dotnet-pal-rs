from pathlib import Path
import sys
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))
from audit_dependencies import forbidden_references
class ForbiddenImportsTests(unittest.TestCase):
    def test_strong_weak_and_versioned_symbols(self):
        r={'unresolved_strong':[{'symbol':'pthread_self@GLIBC_2.2.5','owners':['gcwks']}],
           'unresolved_weak':{'pthread_self':['other'], 'mmap':['vm']}}
        result=forbidden_references(r,['pthread_self'])
        self.assertEqual([x['binding'] for x in result],['strong','weak'])
        self.assertEqual(forbidden_references(r,[]),[])
        self.assertEqual(forbidden_references(r,['pthread']),[])
    def test_missing_symbol_does_not_claim_full_isolation(self):
        r={'unresolved_strong':[{'symbol':'syscall','owners':['other']}], 'unresolved_weak':{}}
        self.assertEqual(forbidden_references(r,['pthread_self']),[])
        self.assertEqual(len(forbidden_references(r,['syscall'])),1)
if __name__=='__main__': unittest.main()
