/* CRT primitives for the freestanding Wasm C/Rust link test. No host imports.
 * Compile with -fno-builtin to prevent recursive lowering into these functions.
 */
#include <stddef.h>
void *memset(void *p, int c, size_t n) {
    unsigned char *d = p;
    for (size_t i = 0; i < n; ++i) d[i] = (unsigned char)c;
    return p;
}
void *memcpy(void *p, const void *s, size_t n) {
    unsigned char *d = p; const unsigned char *b = s;
    for (size_t i = 0; i < n; ++i) d[i] = b[i];
    return p;
}
void *memmove(void *p, const void *s, size_t n) {
    unsigned char *d = p; const unsigned char *b = s;
    if ((size_t)d < (size_t)b) { for (size_t i = 0; i < n; ++i) d[i] = b[i]; }
    else { while (n) { --n; d[n] = b[n]; } }
    return p;
}
int memcmp(const void *a, const void *b, size_t n) {
    const unsigned char *x = a, *y = b;
    for (size_t i = 0; i < n; ++i) if (x[i] != y[i]) return (int)x[i] - (int)y[i];
    return 0;
}
