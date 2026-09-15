// Included instead of the pinned Unix GCEvent implementation, only for NativeAOT.
#include "common.h"
#include "gcenv.structs.h"
#include "gcenv.base.h"
#include "gcenv.os.h"
#include "kernel_adapter.h"

GCEvent::GCEvent() : m_impl(nullptr) {}
void GCEvent::CloseEvent() {
    dotnet_pal_kernel::must(dotnet_pal_kernel::require()->event_destroy(static_cast<void *>(m_impl)));
    m_impl = nullptr;
}
void GCEvent::Set() { dotnet_pal_kernel::must(dotnet_pal_kernel::require()->event_set(static_cast<void *>(m_impl))); }
void GCEvent::Reset() { dotnet_pal_kernel::must(dotnet_pal_kernel::require()->event_reset(static_cast<void *>(m_impl))); }
uint32_t GCEvent::Wait(uint32_t timeout, bool alertable) {
    // The pinned Unix implementation also has no APC/alertable-wait semantics.
    (void)alertable;
    return dotnet_pal_kernel::wait_ms(static_cast<void *>(m_impl), timeout);
}
bool GCEvent::CreateAutoEventNoThrow(bool initial) { return CreateOSAutoEventNoThrow(initial); }
bool GCEvent::CreateManualEventNoThrow(bool initial) { return CreateOSManualEventNoThrow(initial); }
bool GCEvent::CreateOSAutoEventNoThrow(bool initial) {
    if (m_impl) return false;
    void *h = nullptr;
    if (dotnet_pal_kernel::require()->event_create(0, initial ? 1u : 0u, &h) != DOTNET_PAL_OK) return false;
    // Impl is used only as an opaque handle carrier: never constructed/dereferenced.
    m_impl = static_cast<Impl *>(h); return true;
}
bool GCEvent::CreateOSManualEventNoThrow(bool initial) {
    if (m_impl) return false;
    void *h = nullptr;
    if (dotnet_pal_kernel::require()->event_create(1, initial ? 1u : 0u, &h) != DOTNET_PAL_OK) return false;
    m_impl = static_cast<Impl *>(h); return true;
}
