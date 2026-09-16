"""Route the vendored unwinder lock through existing neutral mutex capability."""
from kernel_patch import once
FILE = 'src/native/external/llvm-libunwind/src/RWMutex.hpp'
def transform(text):
    # Include outside libunwind namespace; the remaining ABI/metadata stays upstream.
    text = once(text, 'namespace libunwind {', '#ifdef DOTNET_PAL_KERNEL\n#include "unwind_lock.h"\n#endif\n\nnamespace libunwind {')
    anchor = '#if defined(_LIBUNWIND_HAS_NO_THREADS)\n\nclass'
    replacement = '#if defined(DOTNET_PAL_KERNEL)\nusing RWMutex = DotnetPalUnwindLock;\n#elif defined(_LIBUNWIND_HAS_NO_THREADS)\n\nclass'
    return once(text, anchor, replacement)
