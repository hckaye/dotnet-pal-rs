import sys
from pathlib import Path
import unittest
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'integration/dotnet10'))
import context_patch
class ContextPatchTests(unittest.TestCase):
    def test_independent_guards_preserve_existing_runtime_guards(self):
        source='#ifdef DOTNET_PAL_RUNTIME\nexisting();\n#endif\nvoid Entry()\n{\n original();\n}\n'
        out=context_patch.function(source,'Entry',' replacement();')
        self.assertIn('#ifdef DOTNET_PAL_RUNTIME\nexisting();',out)
        self.assertIn('#ifdef DOTNET_PAL_NATIVE_CONTEXT\n replacement();',out)
        self.assertIn(' original();',out)
    def test_context_patch_rejects_missing_serviced_source(self):
        with self.assertRaises(ValueError):context_patch.pal('')
    def test_thread_handle_remains_target_typed(self):
        out=context_patch.thread('    m_hOSThread = pthread_self();')
        self.assertIn('static_cast<pthread_t>',out)
        self.assertIn('pthread_self()',out)
