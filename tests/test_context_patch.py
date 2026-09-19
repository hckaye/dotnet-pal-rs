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
    def test_hardware_faults_fall_back_to_port_reporting(self):
        source=('static PHARDWARE_EXCEPTION_HANDLER g_hardwareExceptionHandler = NULL;\n#define EXCEPTION_CONTINUE_EXECUTION (-1)\n'
                '// Initialize hardware exception handling\nbool InitializeHardwareExceptionHandling()\n{\n    return install();\n}\n')
        out=context_patch.hardware(source)
        # The adapter must follow the state it reads and precede the function that calls it.
        self.assertLess(out.index('g_hardwareExceptionHandler = NULL'),out.index('#include "faults_adapter.inl"'))
        self.assertLess(out.index('#include "faults_adapter.inl"'),out.index('bool InitializeHardwareExceptionHandling()'))
        self.assertIn('if (!dotnet_pal_context::available()) return dotnet_pal_faults::initialize();',out)
        self.assertIn('    return install();',out)
        with self.assertRaises(ValueError):context_patch.hardware('bool InitializeHardwareExceptionHandling()\n{\n}\n')

    def test_ci_sparse_source_includes_all_patch_inputs(self):
        import patch_runtime,kernel_patch,support_patch,topology_patch,image_patch,minipal_patch
        workflow=(Path(__file__).resolve().parents[1]/'.github/workflows/ci.yml').read_text()
        for name in [patch_runtime.GC_FILE,patch_runtime.CMAKE_FILE,*kernel_patch.FILES,*context_patch.FILES,*support_patch.FILES,
                     *topology_patch.FILES,*image_patch.FILES,*minipal_patch.FILES]:
            self.assertIn('/'+name,workflow,'CI sparse checkout would omit '+name)

    def test_gc_identity_uses_token_and_preserves_other_runtime_paths(self):
        source = """#ifdef TARGET_UNIX
class EEThreadId
{
    bool IsCurrentThread() { return m_isValid && pthread_equal(m_id, pthread_self()); }
    void SetToCurrentThread() { m_id = pthread_self(); m_isValid = true; }
};
#else
class EEThreadId
{
    bool IsCurrentThread() { return windows_identity(); }
};
#endif
"""
        out = context_patch.gc_structs(source)
        self.assertIn('dotnet_pal::ThreadIdentity<dotnet_pal_context::thread_token>', out)
        self.assertIn('class EEThreadId final : public', out)
        self.assertNotIn('using EEThreadId =', out)
        self.assertIn('return windows_identity()', out)
        self.assertIn('pthread_equal(m_id, pthread_self())', out)
        for invalid in (out, source.replace('m_id = pthread_self()', 'm_id = 0'), source+source):
            with self.assertRaises(ValueError): context_patch.gc_structs(invalid)
