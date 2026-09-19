// C++ allocation operators and std::nothrow over the shim's malloc/free.
// -fno-exceptions: the throwing forms return nullptr on failure instead.
#include "freestanding.h"

namespace std {
struct nothrow_t { explicit nothrow_t() = default; };
extern const nothrow_t nothrow;
const nothrow_t nothrow{};
}

void *operator new(size_t n) { return FS_NAME(malloc)(n); }
void *operator new[](size_t n) { return FS_NAME(malloc)(n); }
void *operator new(size_t n, const std::nothrow_t &) noexcept { return FS_NAME(malloc)(n); }
void *operator new[](size_t n, const std::nothrow_t &) noexcept { return FS_NAME(malloc)(n); }
void operator delete(void *p) noexcept { FS_NAME(free)(p); }
void operator delete[](void *p) noexcept { FS_NAME(free)(p); }
void operator delete(void *p, size_t) noexcept { FS_NAME(free)(p); }
void operator delete[](void *p, size_t) noexcept { FS_NAME(free)(p); }
void operator delete(void *p, const std::nothrow_t &) noexcept { FS_NAME(free)(p); }
void operator delete[](void *p, const std::nothrow_t &) noexcept { FS_NAME(free)(p); }

#if defined(FREESTANDING_SELFTEST)
// Exercised by selftest.c; not part of the bare-metal archive.
extern "C" int FS_NAME(cxx_selftest)(void) {
    int *a = new int(7);
    long *b = new (std::nothrow) long[4];
    char *c = new char[3];
    if (!a || !b || !c || *a != 7) return 0;
    b[3] = 9;
    delete a;
    delete[] b;
    ::operator delete[](c, static_cast<size_t>(3));
    return 1;
}
#endif
