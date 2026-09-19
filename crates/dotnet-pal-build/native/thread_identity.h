#ifndef DOTNET_PAL_THREAD_IDENTITY_H
#define DOTNET_PAL_THREAD_IDENTITY_H
#include <cstdint>
namespace dotnet_pal {
// Tokens identify live native threads. They may be recycled after thread exit;
// this is an ownership check, not a persistent globally unique identifier.
// Mutating an identity and testing it concurrently requires caller synchronization.
template<uintptr_t (*Current)()>
class ThreadIdentity {
    uintptr_t id = 0;
    bool valid = false;
public:
    bool IsCurrentThread() { return valid && id == Current(); }
    void SetToCurrentThread() { id = Current(); valid = true; }
    void Clear() { valid = false; }
};
}
#endif
