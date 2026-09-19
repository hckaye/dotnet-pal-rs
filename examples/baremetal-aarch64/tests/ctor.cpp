// Proves the boot code runs .init_array: the .NET runtime archive is full of C++
// objects with constructors and nothing else would run them on this machine.
// Compiled with -fno-exceptions -fno-rtti and with no destructor, so the object
// needs neither a personality routine nor __cxa_atexit.
//
// The constructor reads a volatile, which the compiler may not fold into a
// constant initializer. Without a working .init_array the value stays zero.
namespace {
volatile unsigned seed = 0x5a5a;
struct Probe {
    Probe() { value = seed + 1; }
    unsigned value;
};
Probe probe;
} // namespace

extern "C" unsigned pal_constructor_value() { return probe.value; }
