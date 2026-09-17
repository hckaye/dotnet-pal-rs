#include "support_adapter.h"
#include <cassert>
#include <cstdlib>
#include <thread>
#include <vector>
static dotnet_pal_api api{};
static uint64_t clock_ns;
static std::atomic<unsigned> creates{0},destroys{0},reads{0};
static uint32_t alloc(size_t n,uint32_t,void **out){*out=std::malloc(n);return *out?0:4;}
static uint32_t resize(void *p,size_t n,void **out){*out=std::realloc(p,n);return *out?0:4;}
static uint32_t release(void *p){std::free(p);return 0;}
static uint32_t rwcreate(void **out){creates++;return alloc(8,0,out);}
static uint32_t rwdestroy(void *p){destroys++;return release(p);}
static uint32_t rwread(void *p){assert(p);reads++;return 0;}
static uint32_t write(const uint8_t*,size_t n,size_t *out){*out=n;return 0;}
static uint32_t name(const uint8_t*,size_t n){assert(n<=15);return 0;}
static uint32_t clock(uint64_t *out){*out=clock_ns;return 0;}
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t version){return version==2?&api:nullptr;}
int main(){
    api.header={2,sizeof(api),DOTNET_PAL_CAP_SUPPORT|DOTNET_PAL_CAP_REALTIME};
    api.support={alloc,resize,release,rwcreate,rwread,rwread,rwread,rwdestroy,write,name,nullptr};
    api.runtime.realtime_ns=clock;
    assert(dotnet_pal_support::initialize());
    assert(dotnet_pal_support::filetime()==UINT64_C(116444736000000000));
    clock_ns=12345;assert(dotnet_pal_support::filetime()==UINT64_C(116444736000000123));
    clock_ns=UINT64_MAX;assert(dotnet_pal_support::filetime()==UINT64_C(116444736000000000)+UINT64_MAX/100);
    void *p=dotnet_pal_support::allocate(0);assert(p);dotnet_pal_support::release(p);
    assert(dotnet_pal_support::name("12345678901234567890"));
    dotnet_pal_support::fatal_message("diagnostic");
    static dotnet_pal_support::ReadWriteLock lock;
    std::vector<std::thread> threads;
    for(unsigned i=0;i<8;i++)threads.emplace_back([&](){assert(lock.lock_shared());assert(lock.unlock_shared());});
    for(auto &t:threads)t.join();
    assert(creates==destroys+1 && reads==16);
    api.header.capabilities=0;assert(!dotnet_pal_support::initialize());
    api.header.capabilities=DOTNET_PAL_CAP_SUPPORT;api.header.struct_size=sizeof(dotnet_pal_header);
    assert(!dotnet_pal_support::initialize());
}
