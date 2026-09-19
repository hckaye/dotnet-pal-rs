#!/usr/bin/env bash
# Builds libfreestanding.a for aarch64-unknown-none-elf, links the NativeAOT
# runtime, the bootstrapper, a compiled managed sample and libgcc with every
# unresolved symbol tolerated, and classifies what is still unresolved into
# artifacts/freestanding/unresolved.txt. Run inside the Linux arm64 container;
# the SDK symlinks under artifacts/source-sdk resolve only there.
set -euo pipefail
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$dir/../.." && pwd)"
out="${FREESTANDING_OUT:-$root/artifacts/freestanding}"
mkdir -p "$out/obj"
cc="${CC:-clang}"; cxx="${CXX:-clang++}"; ar="${AR:-llvm-ar}"; ld="${LD:-ld.lld}"; nm="${NM:-llvm-nm}"
target=aarch64-unknown-none-elf
common=(--target=$target -ffreestanding -fno-builtin -nostdlib -O2 -Wall -Wextra -Werror "-I$root/include" "-I$dir")
objects=()
for unit in string strtol printf scanf crt heap; do
  "$cc" -std=c11 "${common[@]}" -c "$dir/$unit.c" -o "$out/obj/$unit.o"
  objects+=("$out/obj/$unit.o")
done
"$cxx" -std=c++17 -fno-exceptions -fno-rtti -nostdinc++ "${common[@]}" -c "$dir/cxxabi.cpp" -o "$out/obj/cxxabi.o"
objects+=("$out/obj/cxxabi.o")
rm -f "$out/libfreestanding.a"
"$ar" rcs "$out/libfreestanding.a" "${objects[@]}"

sdk="$root/artifacts/source-sdk"
managed="$root/samples/GcProbe/obj/Release/net10.0/linux-arm64/native/GcProbe.o"
libgcc="$(ls /usr/lib/gcc/aarch64-linux-gnu/*/libgcc.a | head -n 1)"
inputs=("$managed" "$sdk/libbootstrapper.o" "$sdk/libRuntime.WorkstationGC.a" "$sdk/libeventpipe-disabled.a"
        "$sdk/libstandalonegc-disabled.a" "$out/libfreestanding.a" "$libgcc")
"$ld" -nostdlib -static --gc-sections -e main --unresolved-symbols=ignore-all --warn-unresolved-symbols \
  -Map="$out/link.map" -o "$out/link-probe.elf" "${inputs[@]}" 2> "$out/link.log" || { cat "$out/link.log"; exit 1; }

# Unresolved = undefined in the final image, read from the linked output.
"$nm" -P -u "$out/link-probe.elf" | awk '{print $1}' | sort -u > "$out/unresolved-raw.txt"
# Undefined entries of linked members that no input defines and that are absent
# from the image: either no relocation uses them (lld drops such symbols from
# the output table) or the referencing section was discarded by --gc-sections.
for input in "${inputs[@]}"; do "$nm" -A -P "$input" 2>/dev/null; done \
  | awk '$3 != "U" && $3 != "w" && $3 != "v" {print $2}' | sort -u > "$out/defined.txt"
for input in "${inputs[@]}"; do "$nm" -A -P -u "$input" 2>/dev/null; done \
  | awk '{ sub(/:$/, "", $1); print $1, $2 }' | sort -u > "$out/input-undefined.txt"

if grep -q 'lse-init' "$out/link.map"; then echo "libgcc lse-init.o was linked (getauxval constructor)"; exit 1; fi
if grep -qw getauxval "$out/unresolved-raw.txt"; then echo "getauxval is referenced"; exit 1; fi

python3 - "$out/unresolved-raw.txt" "$out/unresolved.txt" "$out/link.map" "$out/defined.txt" "$out/input-undefined.txt" <<'PY'
import re, sys
raw, dest, mapfile, defined_file, input_undefined = sys.argv[1:6]
OS = set('''__getdelim __sched_cpualloc __sched_cpucount __sched_cpufree asprintf close closedir dl_iterate_phdr dladdr
dlsym dup2 execv fclose fflush fopen64 fork fprintf fputs fwrite getpid getrlimit64 opendir pipe prctl pthread_self read
readdir64 sched_getaffinity sched_getcpu sched_setaffinity statfs64 stderr stdout syscall sysconf sysinfo waitpid write
clock_gettime nanosleep open64 fsync fileno fgetc time getauxval lrand48 srand48 vfprintf __isoc99_fscanf
pthread_mutex_init pthread_mutex_destroy pthread_mutex_lock pthread_mutex_unlock pthread_mutexattr_init
pthread_mutexattr_destroy pthread_mutexattr_settype'''.split())
LIBM = set('''acos acosf acosh acoshf asin asinf asinh asinhf atan atan2 atan2f atanf atanh atanhf cbrt cbrtf ceil ceilf
cos cosf cosh coshf exp exp2 exp2f expf expm1 expm1f fabs fabsf floor floorf fma fmaf fmod fmodf frexp hypot hypotf
ilogb ldexp log log10 log10f log1p log1pf log2 log2f logb logf modf modff pow powf remainder rint round roundf scalbn sin
sinf sinh sinhf sqrt sqrtf tan tanf tanh tanhf trunc truncf'''.split())
BUILTINS = set('memcpy memmove memset memcmp bcmp'.split())
CRT_PREFIXES = ('str', 'mem', '__cxa_', '__gxx_', '_Unwind_', '__isoc', '__errno', '__stack_chk', 'vsn', 'sn', 'ato',
                '__clear_cache', '_Zn', '_Zd', '_ZSt', 'malloc', 'calloc', 'realloc', 'free', 'posix_memalign',
                'aligned_alloc', 'abort', 'exit', 'atexit', '__dso_handle', '__aarch64_')
def classify(s):
    if s.startswith('SystemNative_'): return 'SystemNative_*'
    if s.startswith('dotnet_pal_'): return 'dotnet_pal_*'
    if s in LIBM: return 'libm'
    if s in BUILTINS: return 'compiler-builtins (Rust)'
    if s in OS or s.startswith('minipal_'): return 'os-service'
    if s in ('dst', 'val'): return 'other'
    if s.startswith(CRT_PREFIXES): return 'crt'
    return 'other'
unresolved = open(raw).read().split()
rows = [(classify(s), s) for s in unresolved]
order = ['SystemNative_*', 'dotnet_pal_*', 'libm', 'compiler-builtins (Rust)', 'os-service', 'crt', 'other']
# Members the link actually used, from the map, e.g. "libRuntime.WorkstationGC.a(WriteBarriers.S.o)".
maptext = open(mapfile).read()
linked = set(re.findall(r'(\S+\.a\([^)]+\)|\S+\.o)', maptext))
linked_members = {m for m in linked}
defined = set(open(defined_file).read().split())
dropped = {}
for line in open(input_undefined):
    owner, sym = line.split()
    member = owner.rsplit('/', 1)[-1].replace('[', '(').replace(']', ')')   # nm prints a[m], the map prints a(m)
    if not any(l.endswith(member) for l in linked_members): continue
    if sym in defined or sym in unresolved: continue
    dropped.setdefault(sym, set()).add(member)
with open(dest, 'w') as f:
    for cat in order:
        names = sorted(s for c, s in rows if c == cat)
        f.write(f'[{cat}] ({len(names)})\n')
        for s in names: f.write(f'  {s}\n')
    f.write(f'[absent from the image: undefined in a linked member, defined by no input, no relocation or section discarded] ({len(dropped)})\n')
    for s in sorted(dropped): f.write(f'  {s}  [{classify(s)}]  <- {", ".join(sorted(dropped[s]))}\n')
crt = [s for c, s in rows if c == 'crt'] + [s for s in dropped if classify(s) == 'crt']
print(open(dest).read())
if crt:
    print('CRT symbols still unresolved: ' + ' '.join(sorted(crt))); sys.exit(1)
print('LINK ENUMERATION OK: no crt symbols unresolved')
PY
