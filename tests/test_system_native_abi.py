"""native/system_native_abi.h must equal the pinned System.Native headers.

Runs when a dotnet/runtime checkout is available (DOTNET_PAL_RUNTIME_SOURCE or
artifacts/runtime-source); otherwise the comparison is skipped, never faked.
"""
import os
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HEADER = ROOT / 'native/system_native_abi.h'


def runtime_root():
    for candidate in (os.environ.get('DOTNET_PAL_RUNTIME_SOURCE'), ROOT / 'artifacts/runtime-source', '/runtime-src'):
        if candidate and (Path(candidate) / 'src/native/libs/System.Native/pal_io.h').is_file():
            return Path(candidate)
    return None


def enum_values(text, names):
    found = {}
    for name in names:
        m = re.search(rf'\b{re.escape(name)}\s*=\s*(0x[0-9A-Fa-f]+|-?\d+)', text)
        if m: found[name] = int(m.group(1), 0)
    return found


def struct_fields(text, name):
    m = re.search(r'typedef struct\s*(?:\w+\s*)?\{([^}]*)\}\s*' + re.escape(name) + r'\s*;', text, re.S)
    if not m: return None
    body = re.sub(r'//[^\n]*', '', m.group(1))
    return [f.strip() for f in body.replace('\n', ' ').split(';') if f.strip()]


class SystemNativeAbiTests(unittest.TestCase):
    def setUp(self):
        self.root = runtime_root()
        if not self.root: self.skipTest('no pinned dotnet/runtime checkout available')
        self.ours = HEADER.read_text()
        libs = self.root / 'src/native/libs'
        self.upstream = '\n'.join((libs / p).read_text() for p in (
            'Common/pal_error_common.h', 'Common/pal_io_common.h', 'Common/pal_networking_common.h', 'System.Native/pal_io.h',
            'System.Native/pal_console.h', 'System.Native/pal_time.h', 'System.Native/pal_uid.h', 'System.Native/pal_process.h',
            'System.Native/pal_networking.h', 'System.Native/pal_mount.h', 'System.Native/pal_interfaceaddresses.h',
            'System.Native/pal_maphardwaretype.h'))

    def test_every_constant_matches_the_pinned_headers(self):
        # Literal values only: SO_EXCLUSIVEADDRUSE is written as the complement of SO_REUSEADDR here and upstream.
        names = re.findall(r'\b((?:Error_|PAL_|FILESTATUS_FLAGS_|AddressFamily_|SocketType_|ProtocolType_|SocketShutdown_|SocketOptionLevel_|'
                           r'SocketOptionName_|SocketFlags_|SocketEvents_|GetAddrInfoErrorFlags_|NUM_BYTES_IN_|MAX_IP_ADDRESS_|MulticastOption_|OperationalStatus_|'
                           r'NetworkInterfaceType_)\w+)\s*=\s*(?:0x[0-9A-Fa-f]+|-?\d+)', self.ours)
        ours = enum_values(self.ours, names)
        theirs = enum_values(self.upstream, names)
        self.assertEqual(sorted(ours), sorted(names))
        for name in names:
            self.assertIn(name, theirs, f'{name} is not in the pinned headers')
            self.assertEqual(ours[name], theirs[name], name)

    def test_structures_have_the_pinned_layout(self):
        for name in ('FileStatus', 'PollEvent', 'WinSize', 'TimeSpec', 'ProcessCpuInformation', 'Passwd', 'IOVector', 'DirectoryEntry',
                     'IPAddress', 'HostEntry', 'IPPacketInformation', 'IPv4MulticastOption', 'IPv6MulticastOption', 'LingerOption',
                     'MessageHeader', 'SocketEvent', 'MountPointInformation', 'LinkLayerAddressInfo', 'IpAddressInfo', 'NetworkInterfaceInfo'):
            self.assertEqual(struct_fields(self.ours, name), struct_fields(self.upstream, name), name)


if __name__ == '__main__':
    unittest.main()
