"""Opt-in initial-exec TLS for statically linked/initial-load NativeAOT images.

The general-dynamic path is retained for other consumers. Both C/C++ and the
hand-written x64 assembly must agree; compiler flags alone do not patch assembly.
"""
from kernel_patch import once

AMD64 = "src/coreclr/nativeaot/Runtime/unix/unixasmmacrosamd64.inc"
FILES = (AMD64,)


def amd64(text):
    if "DOTNET_PAL_STATIC_TLS" in text:
        raise ValueError("x64 TLS assembly is already patched")
    old = """        callq   *(%rdi)
#else
        .byte 0x66  // data16 prefix - padding to have space for linker relaxations"""
    new = r"""        callq   *(%rdi)
#elif defined(DOTNET_PAL_STATIC_TLS)
        // Initial-exec TLS: the initial image owns this thread's static TLS.
        // Return the address in RAX, just as __tls_get_addr does, without a call.
        movq    \Var@GOTTPOFF(%rip), %rax
        addq    %fs:0, %rax
#else
        .byte 0x66  // data16 prefix - padding to have space for linker relaxations"""
    return once(text, old, new)


TRANSFORMS = {AMD64: amd64}
