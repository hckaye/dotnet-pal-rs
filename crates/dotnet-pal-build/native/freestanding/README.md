# Freestanding C runtime shim

C11 sources (plus one C++ file) that give the .NET NativeAOT runtime archive
the C runtime surface it links against, on a target that has no libc and no
operating system. Nothing in this directory calls an OS. The only outside
contracts are:

- `dotnet_pal_get_api(DOTNET_PAL_ABI_VERSION)` from `include/dotnet_pal.h`:
  the `support` group's `allocate`/`resize`/`release` back the malloc family.
- `dotnet_pal_freestanding_abort(void)` and `dotnet_pal_freestanding_exit(int)`,
  both `_Noreturn`, defined by the port. `abort`, `__stack_chk_fail`,
  `__cxa_pure_virtual` and a recursive `__cxa_guard_acquire` end in the first;
  `exit` (after the handlers) and `_Exit` end in the second.
- `memcpy`, `memmove`, `memset`, `memcmp`, `bcmp`: not defined here. The
  bare-metal port gets them from Rust's `compiler_builtins`; the host self-test
  gets them from glibc.
- libm (`log`, `cos`, `pow`, `sin`, ...): not defined here; the port supplies it.

These files are the CRT contract for OS-less targets: everything an OS would
normally provide is either absent (and routed through the boundary elsewhere)
or listed above as a hook.

## Build

Target build (what `link-enumerate.sh` does):

```
clang --target=aarch64-unknown-none-elf -std=c11 -ffreestanding -fno-builtin -nostdlib -O2 \
      -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native/freestanding -c string.c strtol.c printf.c scanf.c crt.c heap.c
clang++ --target=aarch64-unknown-none-elf -std=c++17 -fno-exceptions -fno-rtti -nostdinc++ \
      -ffreestanding -fno-builtin -nostdlib -O2 -Wall -Wextra -Werror -Iinclude -Icrates/dotnet-pal-build/native/freestanding -c cxxabi.cpp
llvm-ar rcs libfreestanding.a *.o
```

Every exported definition is spelled `FS_NAME(name)`; `-DFREESTANDING_PREFIX=fs_`
renames the entire surface (`fs_strlen`, `fs___errno_location`, ...) so the
sources can be linked next to a real libc. `freestanding.h` has one prototype
line per symbol and is the list of what the archive exports. The C++ operators
and `std::nothrow` cannot be renamed and stay as they are.

`__clear_cache` contains AArch64 inline assembly and refuses to compile for
any other architecture.

## What is provided

| File | Symbols |
| --- | --- |
| `string.c` | `strlen strnlen strcmp strncmp strcasecmp strncasecmp strcpy strncpy strcat strncat strchr strrchr strstr strspn strcspn strpbrk strtok_r strdup strndup memchr strerror strerror_r __xpg_strerror_r` |
| `strtol.c` | `strtol strtoul strtoll strtoull atoi atol atoll` and the glibc 2.38+ names `__isoc23_strtol __isoc23_strtoul __isoc23_strtoll __isoc23_strtoull` |
| `printf.c` | `vsnprintf snprintf vsprintf sprintf` |
| `scanf.c` | `vsscanf sscanf __isoc99_vsscanf __isoc99_sscanf __isoc23_vsscanf __isoc23_sscanf` |
| `crt.c` | `__errno_location abort exit _Exit atexit __cxa_atexit __cxa_finalize __dso_handle __cxa_pure_virtual __cxa_deleted_virtual __cxa_guard_acquire __cxa_guard_release __cxa_guard_abort __stack_chk_guard __stack_chk_fail __aarch64_have_lse_atomics __clear_cache` |
| `heap.c` | `malloc calloc realloc free posix_memalign aligned_alloc memalign` |
| `cxxabi.cpp` | `operator new`, `operator new[]`, their `nothrow` forms, `operator delete`, `operator delete[]` (plain, sized and `nothrow`), `std::nothrow` |

## Behavior and limits

Strings

- Byte semantics, C locale: `strcasecmp` folds ASCII only.
- `strerror` returns `"error <n>"` from a thread-local buffer; there is no
  message table. `strerror_r` follows the GNU convention (returns a string,
  `buf` when the text fits) because that is the symbol glibc headers bind to
  under `_GNU_SOURCE`; `__xpg_strerror_r` is the XSI form (0 or `ERANGE`).
- `strdup`/`strndup` allocate with the shim's `malloc`.

Number parsing

- `strto*`: whitespace, sign, `0x` prefix for base 0/16, octal for a leading
  `0` in base 0, bases 2..36, `ERANGE` clamping, `EINVAL` for a bad base with
  `*endptr` left untouched (as glibc does). "-1" in the unsigned variants wraps
  without `ERANGE`, as in glibc.
- The `__isoc23_*` names additionally accept a `0b`/`0B` prefix for base 0 and
  base 2, matching what glibc's C23 entry points do.

Formatting (`vsnprintf`)

- Flags `- + space 0 #`, width and precision (digits or `*`), length modifiers
  `hh h l ll z t j L q`, conversions `d i u x X o c s p n %` and `f F e E g G`.
- Floating output is an exact binary-to-decimal expansion rounded half to even
  on the exact value, which is what glibc prints in the default rounding mode;
  the self-test checks byte equality against glibc for every case it runs.
  Limits: at most 512 fractional (`%f`) or significant (`%e`, `%g`) digits are
  computed and later positions print as `0`; `%L` narrows `long double` to
  `double` before formatting (this uses libgcc's `__trunctfdf2`).
- Not supported: `%a`, positional `%1$` arguments, `%m`, locale grouping.
- `%p` prints `(nil)` for NULL and `0x` + hex otherwise; `%s` with NULL prints
  `(null)` unless the precision is below 6; both as glibc.

Scanning (`sscanf`)

- Conversions `d i u o x X p s c n %`, `*` suppression, a width, length
  modifiers `hh h l ll z t j q`. `%c` with a width stores a short read at end
  of input and counts it, as glibc does.
- Not supported: floating conversions, `%[` scan sets, positional arguments;
  the first such specifier ends the scan and the count so far is returned.
- `__isoc23_sscanf` differs only in `%i` accepting a `0b` prefix.

CRT

- `errno` lives in a `_Thread_local int` behind `__errno_location`; the port
  sets up the ELF TLS block.
- `exit` runs `atexit`/`__cxa_atexit` handlers in reverse order (handlers
  registered during exit also run), then calls the port's exit hook. The table
  holds 64 handlers; registration beyond that fails with -1.
  `__cxa_finalize(dso)` runs the handlers registered for that `dso`.
- `__cxa_guard_*` implement the 64-bit Itanium guard for cooperative threads:
  no waiting, and a second acquirer while an initialization is in progress
  aborts (that is a recursive initialization).
- `__stack_chk_guard` is a fixed nonzero constant; nothing runs early enough
  to seed it.
- `__aarch64_have_lse_atomics` is a zero byte (libgcc declares it `_Bool`),
  so libgcc's outline atomics take the LL/SC path and `lse-init.o` with its
  `getauxval` constructor is never linked. `link-enumerate.sh` checks this.
- `__clear_cache(begin, end)`: `dc cvau` per data line, `dsb ish`, `ic ivau`
  per instruction line, `dsb ish`, `isb`; line sizes and the IDC/DIC shortcuts
  come from `CTR_EL0`.

Heap

- Backed by the boundary's native heap. The table is fetched on first use and
  must carry `DOTNET_PAL_CAP_NATIVE_HEAP`; while it is unavailable every
  allocation returns NULL with `errno = ENOMEM` and nothing traps.
- Each block has a 16-byte header just below the returned pointer (provider
  pointer, size, alignment). `free` and `realloc` therefore never read outside
  the block, and over-aligned blocks (`posix_memalign`, `aligned_alloc`,
  `memalign`) need no separate bookkeeping. Provider memory is treated as
  16-byte aligned; if it is only pointer aligned the header math still yields
  16-byte aligned results. `realloc` of an over-aligned block allocates, copies
  and frees; `realloc` of an ordinary block uses `resize` and a failed resize
  leaves the old block valid.
- `malloc(0)` returns a distinct 1-byte block; `realloc(p, 0)` frees `p` and
  returns NULL; `aligned_alloc` does not require the size to be a multiple of
  the alignment (glibc behavior).

## Verification

`selftest.sh` (run inside the `dotnet-pal-baremetal` container) compiles the
shim for the host with the `fs_` prefix, links `selftest.c` against glibc and
compares every function with glibc on the same inputs, using a stub
`dotnet_pal_get_api` bump allocator for the heap. It prints `SELFTEST PASS`
and exits 0 when every check holds.

`link-enumerate.sh` (same container) builds `artifacts/freestanding/libfreestanding.a`
for `aarch64-unknown-none-elf`, links the managed sample, the bootstrapper,
the runtime archive, the disabled eventpipe/standalone-GC archives, this
library and libgcc with unresolved symbols tolerated, and writes
`artifacts/freestanding/unresolved.txt` with each remaining symbol classified.
The `crt` class must be empty; the script fails otherwise, and also fails if
`lse-init.o` or `getauxval` appears.
