# Protecting the NativeAOT image cookie

`InitGSCookie` in the pinned runtime's `startup.cpp` temporarily changes the page
containing `__security_cookie` from R to RW, writes it, and restores R. The cookie
is loaded in `.rodata`, not obtained from the port allocator. Rejecting every
address below `__heap_start` therefore prevents NativeAOT startup.

The bare-metal linker now isolates `.rodata` on 4 KiB boundaries, excluding code,
the GOT, mutable data, page tables, TLS and the boot stack. The protection helper
allows only R and RW on ranges wholly within this dedicated segment. It retains
its normal permissions for allocator-region mappings, and rejects image no-access
or executable requests without altering descriptors. Commit is still limited to
the allocator; decommit/release cannot revoke image pages or add them to its budget.

The constructor probe executes R/RW/R on an assembly-defined image cookie before
`main`, checks its value and AP/PXN descriptors, and rejects mixed ranges and
attempts to protect code, page tables or boot stack. The original table tests
continue to require CPU faults for RO/NX/no-access allocator mappings. The managed
Console/IO/System/Facilities images remain separate end-to-end qualification.

This is not full image W^X hardening: boot mapping permissions outside the pages
explicitly protected are unchanged. It does not add stack guards or expand the
single-core/cooperative execution model.
