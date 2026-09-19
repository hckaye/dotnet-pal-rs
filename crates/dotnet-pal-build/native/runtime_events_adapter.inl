// Preserve the pinned runtime's UnixEvent C++ interface without pthread layout.
class UnixEvent {
    void *m_handle = nullptr;
    bool m_manual;
    bool m_initial;
public:
    UnixEvent(bool manual, bool initial) : m_manual(manual), m_initial(initial) {}
    bool Initialize() {
        if (m_handle) return false;
        return dotnet_pal_kernel::require()->event_create(m_manual ? 1u : 0u, m_initial ? 1u : 0u, &m_handle) == DOTNET_PAL_OK;
    }
    bool Destroy() {
        if (!m_handle) return true;
        if (dotnet_pal_kernel::require()->event_destroy(m_handle) != DOTNET_PAL_OK) return false;
        m_handle = nullptr; return true;
    }
    uint32_t Wait(uint32_t milliseconds, bool alertable = false) {
        (void)alertable;
        return dotnet_pal_kernel::wait_ms(m_handle, milliseconds);
    }
    void Set() { dotnet_pal_kernel::must(dotnet_pal_kernel::require()->event_set(m_handle)); }
    void Reset() { dotnet_pal_kernel::must(dotnet_pal_kernel::require()->event_reset(m_handle)); }
};
