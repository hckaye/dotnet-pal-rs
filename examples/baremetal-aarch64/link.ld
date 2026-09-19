/* Static non-PIE image for QEMU -M virt with -m 1024.
 *
 * RAM is 0x4000_0000 .. 0x8000_0000. QEMU's -kernel loader places an ELF at its
 * link addresses, so the image starts at the usual 0x4008_0000. Everything after
 * the image up to the end of RAM is the static region the port's providers carve
 * memory from; there is no OS and no other owner of that RAM.
 */
ENTRY(_start)

__ram_end = 0x80000000;

SECTIONS
{
    . = 0x40080000;
    __image_start = .;

    .text : {
        __text_start = .;
        KEEP(*(.text.boot))
        KEEP(*(.text.vectors))
        *(.text .text.*)
        __text_end = .;
    }

    /* InitGSCookie temporarily writes __security_cookie in .rodata. Keep
     * these pages separate from code and mutable loader/runtime data so the
     * port can restore READ-only access without changing neighbouring objects. */
    .rodata : ALIGN(4096) {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
        /* The NativeAOT exception tables of the managed code. */
        KEEP(*(.dotnet_eh_table))
        . = ALIGN(4096);
        __rodata_end = .;
    }
    /* Global offset table of the position-independent runtime and managed objects,
     * resolved statically by the linker. */
    .got : ALIGN(8) { *(.got .got.plt) }

    /* The .NET unwinder locates the DWARF tables through these symbols; nothing
     * in this example unwinds, but the layout a runtime link needs is here. */
    .eh_frame_hdr : ALIGN(8) {
        __eh_frame_hdr_start = .;
        KEEP(*(.eh_frame_hdr))
        __eh_frame_hdr_end = .;
    }
    .eh_frame : ALIGN(8) {
        __eh_frame_start = .;
        KEEP(*(.eh_frame))
        __eh_frame_end = .;
    }

    .data : ALIGN(16) {
        *(.data.rel.ro .data.rel.ro.*)
        *(.data .data.*)
        *(.sdata .sdata.*)
    }
    .fini_array : ALIGN(8) { KEEP(*(.fini_array .fini_array.*)) }

    /* C++ static constructors, run by the boot code before main. */
    .init_array : ALIGN(8) {
        __init_array_start = .;
        KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
        KEEP(*(.init_array))
        __init_array_end = .;
    }

    /* The NativeAOT bootstrapper reads __start___modules/__stop___modules and the
     * two other pairs. lld defines those symbols for output sections with exactly
     * these names, so they must not be folded into .data or .rodata. */
    __modules : ALIGN(8) { KEEP(*(__modules)) }
    __managedcode : ALIGN(8) { KEEP(*(__managedcode)) }
    __unbox : ALIGN(8) { KEEP(*(__unbox)) }

    /* PT_TLS image. The port copies .tdata and zeroes .tbss into a per-thread
     * block; see src/tls.rs for the aarch64 variant I layout it builds. */
    . = ALIGN(16);
    .tdata : {
        __tdata_start = .;
        KEEP(*(.tdata .tdata.*))
        __tdata_end = .;
    }
    .tbss : {
        KEEP(*(.tbss .tbss.*))
        *(.tcommon)
        __tbss_end = .;
    }

    .bss (NOLOAD) : ALIGN(16) {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        /* ILCompiler's dehydrated data is expanded here at startup. */
        *(.hydrated)
        . = ALIGN(16);
        __bss_end = .;
    }

    /* Boot thread stack. Not in any section: the boot code sets SP to the top and
     * .bss zeroing stops before it. */
    . = ALIGN(4096);
    __boot_stack_bottom = .;
    . += 2M;
    __boot_stack_top = .;

    /* The rest of RAM. VirtualMemory, NativeMapping, thread stacks, per-thread TLS
     * blocks and the native heap are all carved from this one region. */
    . = ALIGN(4096);
    __heap_start = .;
    __heap_end = __ram_end;
    __image_end = .;

    /* DWARF, kept for symbolizing crash addresses; none of it is loaded. */
    .debug_abbrev 0 : { *(.debug_abbrev) }
    .debug_addr 0 : { *(.debug_addr) }
    .debug_aranges 0 : { *(.debug_aranges) }
    .debug_frame 0 : { *(.debug_frame) }
    .debug_info 0 : { *(.debug_info) }
    .debug_line 0 : { *(.debug_line) }
    .debug_line_str 0 : { *(.debug_line_str) }
    .debug_loc 0 : { *(.debug_loc) }
    .debug_loclists 0 : { *(.debug_loclists) }
    .debug_macinfo 0 : { *(.debug_macinfo) }
    .debug_macro 0 : { *(.debug_macro) }
    .debug_names 0 : { *(.debug_names) }
    .debug_pubnames 0 : { *(.debug_pubnames) }
    .debug_pubtypes 0 : { *(.debug_pubtypes) }
    .debug_gnu_pubnames 0 : { *(.debug_gnu_pubnames) }
    .debug_gnu_pubtypes 0 : { *(.debug_gnu_pubtypes) }
    .debug_ranges 0 : { *(.debug_ranges) }
    .debug_rnglists 0 : { *(.debug_rnglists) }
    .debug_str 0 : { *(.debug_str) }
    .debug_str_offsets 0 : { *(.debug_str_offsets) }
    .debug_types 0 : { *(.debug_types) }
    .symtab 0 : { *(.symtab) }
    .strtab 0 : { *(.strtab) }
    .shstrtab 0 : { *(.shstrtab) }

    /DISCARD/ : {
        *(.comment)
        *(.note .note.*)
        *(.interp)
        *(.dynamic)
        *(.hash)
        *(.gnu.hash)
        *(.ARM.attributes)
    }
}

/* src/tls.rs assumes a TLS segment alignment of at most 16 bytes, which lets the
 * thread pointer sit exactly 16 bytes (the TCB) below the TLS block. A more
 * strongly aligned thread-local would silently move every offset, so fail here. */
ASSERT(ALIGNOF(.tdata) <= 16, "TLS alignment above 16 is not supported by this port")
ASSERT(ALIGNOF(.tbss) <= 16, "TLS alignment above 16 is not supported by this port")
ASSERT(__heap_start < __heap_end, "the image plus its boot stack does not leave RAM for the port")
