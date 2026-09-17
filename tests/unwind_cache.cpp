// Exercise the ACTUAL patched upstream cache, not a copied algorithm.
#include "UnwindCursor.hpp"
#include <cassert>
#include <cstdlib>
#include <cstdio>
#include <thread>
#include <vector>
static bool fail_allocation;
static unsigned allocations, frees;
static dotnet_pal_api api{};
static uint32_t allocate_bytes(size_t n,uint32_t,void **out){
    if(fail_allocation){*out=nullptr;return DOTNET_PAL_OUT_OF_MEMORY;}
    *out=std::malloc(n);if(!*out)return DOTNET_PAL_OUT_OF_MEMORY;
    allocations++;return 0;
}
static uint32_t resize_bytes(void *p,size_t n,void **out){*out=std::realloc(p,n);return *out?0:4;}
static uint32_t release_bytes(void *p){frees++;std::free(p);return 0;}
extern "C" const dotnet_pal_api *dotnet_pal_get_api(uint32_t version){return version==2?&api:nullptr;}
int main(){
    api.header={2,sizeof(api),DOTNET_PAL_CAP_SUPPORT};
    api.support=dotnet_pal_host_support_v2()->ops;
    api.support.allocate=allocate_bytes;api.support.resize=resize_bytes;api.support.release=release_bytes;
    assert(dotnet_pal_support::initialize());
    using Cache=libunwind::DwarfFDECache<libunwind::LocalAddressSpace>;
    assert(Cache::initialize());
    for(uintptr_t i=0;i<64;i++)Cache::add(100,1000+i*10,1000+i*10+5,2000+i);
    fail_allocation=true;
    Cache::add(100,1640,1645,2064); // growth fails; must retain old cache and unlock
    assert(Cache::findFDE(100,1001)==2000);
    assert(Cache::findFDE(100,1641)==0);
    fail_allocation=false;
    Cache::add(100,1640,1645,2064);
    assert(Cache::findFDE(100,1641)==2064);
    assert(allocations==1 && frees==0);
    std::vector<std::thread> readers;
    for(unsigned t=0;t<8;t++)readers.emplace_back([](){
        for(unsigned i=0;i<1000;i++)assert(Cache::findFDE(100,1001)==2000);
    });
    for(auto &t:readers)t.join();
    Cache::removeAllIn(100);assert(Cache::findFDE(100,1001)==0);
    std::puts("UPSTREAM UNWIND CACHE PASS startup lock, growth OOM preservation/retry, concurrent readers");
}
