// Proves .init_array runs and exercises NativeAOT's image-cookie R -> RW -> R
// sequence before main. No libc, exceptions, RTTI or destructor registration.
#include "dotnet_pal.h"

extern "C" {
void pal_console_write(const uint8_t *, size_t);
void pal_exit(int32_t);
size_t pal_region_free();
extern const char __rodata_start[], __rodata_end[], __boot_stack_bottom[];
extern uint64_t pal_ram_pages[];
extern volatile uint64_t pal_test_image_cookie;
}
// Defined in assembly so C++ never treats the loaded value as a constant.
asm(".pushsection .rodata.pal_image_cookie,\"a\",%progbits\n"
    ".balign 8\n.globl pal_test_image_cookie\npal_test_image_cookie:\n.quad 7\n.popsection\n");

namespace {
void require(bool condition) {
    if (!condition) {
        const char message[] = "FAIL image cookie protection during .init_array\n";
        pal_console_write(reinterpret_cast<const uint8_t *>(message), sizeof(message) - 1);
        pal_exit(1);
    }
}
void image_cookie() {
    const dotnet_pal_api *api = dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION);
    require(api != nullptr);
    const auto &rt = api->runtime;
    const size_t page = 4096;
    const uint32_t rw = DOTNET_PAL_READ | DOTNET_PAL_WRITE;
    uintptr_t base = reinterpret_cast<uintptr_t>(&pal_test_image_cookie) & ~(page - 1);
    uintptr_t low = reinterpret_cast<uintptr_t>(__rodata_start);
    uintptr_t high = reinterpret_cast<uintptr_t>(__rodata_end);
    require(low % page == 0 && high % page == 0 && base >= low && base + page <= high);
    size_t available = pal_region_free();
    size_t index = (base - 0x40000000) / page;
    auto descriptors = static_cast<volatile uint64_t *>(pal_ram_pages);
    // Start with R, as a loader would. The rest of the boot image is not W^X.
    require(rt.mapping_protect(reinterpret_cast<void *>(base), page, DOTNET_PAL_READ) == DOTNET_PAL_OK);
    // AP[2] enforces read-only at EL1, PXN forbids execution.
    require((descriptors[index] & ((UINT64_C(1) << 7) | (UINT64_C(1) << 53)))
            == ((UINT64_C(1) << 7) | (UINT64_C(1) << 53)));
    require(pal_test_image_cookie == 7);
    require(rt.mapping_protect(reinterpret_cast<void *>(base), page, rw) == DOTNET_PAL_OK);
    pal_test_image_cookie = UINT64_C(0x5a5b1234);
    require(pal_test_image_cookie == UINT64_C(0x5a5b1234));
    require(rt.mapping_protect(reinterpret_cast<void *>(base), page, DOTNET_PAL_READ) == DOTNET_PAL_OK);
    require((descriptors[index] & (UINT64_C(1) << 7)) != 0);
    require(api->image.readable(reinterpret_cast<uintptr_t>(&pal_test_image_cookie), sizeof pal_test_image_cookie) == DOTNET_PAL_OK);
    require(rt.mapping_protect(reinterpret_cast<void *>(base), page, 0) == DOTNET_PAL_UNSUPPORTED);
    require(rt.mapping_protect(reinterpret_cast<void *>(base), page, rw | DOTNET_PAL_EXECUTE) == DOTNET_PAL_UNSUPPORTED);
    require(api->vm.decommit(reinterpret_cast<void *>(base), page) == DOTNET_PAL_UNSUPPORTED);
    require(api->vm.commit(reinterpret_cast<void *>(base), page) == DOTNET_PAL_INVALID_ARGUMENT);
    require(rt.mapping_release(reinterpret_cast<void *>(base), page) != DOTNET_PAL_OK);
    require(rt.mapping_protect(reinterpret_cast<void *>(high), page, DOTNET_PAL_READ) == DOTNET_PAL_INVALID_ARGUMENT);
    require(rt.mapping_protect(reinterpret_cast<void *>(base), high - base + page, rw) == DOTNET_PAL_INVALID_ARGUMENT);
    require((descriptors[index] & (UINT64_C(1) << 7)) != 0); // rejected mixed request was atomic
    uintptr_t forbidden[] = {
        reinterpret_cast<uintptr_t>(&image_cookie) & ~(page - 1),
        reinterpret_cast<uintptr_t>(pal_ram_pages),
        reinterpret_cast<uintptr_t>(__boot_stack_bottom)
    };
    for (uintptr_t address : forbidden)
        require(rt.mapping_protect(reinterpret_cast<void *>(address), page, DOTNET_PAL_READ) == DOTNET_PAL_INVALID_ARGUMENT);
    require(pal_region_free() == available);
    const char message[] = "IMAGE DATA PROTECTION PASS cookie R/RW/R and image bounds\n";
    pal_console_write(reinterpret_cast<const uint8_t *>(message), sizeof(message) - 1);
}
volatile unsigned seed = 0x5a5a;
struct Probe {
    Probe() { image_cookie(); value = seed + 1; }
    unsigned value;
};
Probe probe;
} // namespace

extern "C" unsigned pal_constructor_value() { return probe.value; }
