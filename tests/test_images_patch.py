from pathlib import Path
import sys
import unittest
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/'integration/dotnet10'))
import images_patch as patch

class ImagePatches(unittest.TestCase):
    def test_only_exact_callsites_are_replaced(self):
        text='// dladdr is the API\nint a=dladdr(x,y);\n'
        out=patch.callsites(text,'dladdr','replacement',1)
        self.assertIn('int a=DOTNET_PAL_IMAGE_DLADDR(x,y);',out)
        self.assertIn('#else\n#define DOTNET_PAL_IMAGE_DLADDR dladdr',out)
        for n in (0,2):
            with self.assertRaises(ValueError):patch.callsites(text,'dladdr','replacement',n)
    def test_loader_pointer_escape_is_disabled(self):
        text='#if defined(DLFO_STRUCT_HAS_EH_DBASE) && defined(_LIBUNWIND_SUPPORT_DWARF_INDEX) && DLFO_EH_SEGMENT_TYPE == PT_GNU_EH_FRAME\ncall();\n#endif\ndladdr(x,y);\ndl_iterate_phdr(x,y);'
        out=patch.address(text)
        self.assertIn('&& !defined(DOTNET_PAL_IMAGES)',out)
        self.assertIn('dotnet_pal_images::iterate',out)
    def test_no_tls_metadata_is_fabricated(self):
        text=(ROOT/'native/images_adapter.h').read_text()
        self.assertIn('offsetof(dl_phdr_info,dlpi_subs)+sizeof(info.dlpi_subs)',text)
        self.assertNotIn('info.dlpi_tls_data=',text)
    def test_host_getter_is_not_a_runtime_import(self):
        text=(ROOT/'native/images_adapter.h').read_text()
        self.assertNotIn('dotnet_pal_host_images_v2',text)
        self.assertIn('dotnet_pal_get_api',text)

if __name__=='__main__':unittest.main()
