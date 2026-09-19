/* strto* and ato*. The __isoc23_* names are the glibc 2.38+ symbols that
 * objects built against C23-mode headers reference; they differ from the
 * plain names only by accepting a "0b"/"0B" prefix for base 0 and base 2.
 */
#include "freestanding.h"
#include <limits.h>

static int is_space(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }

static int digit_value(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'z') return c - 'a' + 10;
    if (c >= 'A' && c <= 'Z') return c - 'A' + 10;
    return 99;
}

/* Magnitude scan shared by every variant. *end (when given) is set to the
 * first unconsumed byte, or to s when nothing was parsed; an invalid base
 * sets EINVAL and, like glibc, leaves *end alone. */
static unsigned long long scan(const char *s, char **end, int base, int c23, int *negative, int *overflow) {
    const char *p = s;
    *negative = 0; *overflow = 0;
    if (base < 0 || base == 1 || base > 36) { fs_errno = FS_EINVAL; return 0; }
    while (is_space((unsigned char)*p)) ++p;
    if (*p == '+' || *p == '-') { *negative = *p == '-'; ++p; }
    if ((base == 0 || base == 16) && p[0] == '0' && (p[1] == 'x' || p[1] == 'X') && digit_value((unsigned char)p[2]) < 16) {
        p += 2; base = 16;
    } else if (c23 && (base == 0 || base == 2) && p[0] == '0' && (p[1] == 'b' || p[1] == 'B') && digit_value((unsigned char)p[2]) < 2) {
        p += 2; base = 2;
    } else if (base == 0) {
        base = *p == '0' ? 8 : 10;
    }
    unsigned long long acc = 0;
    const char *digits = p;
    for (;; ++p) {
        int d = digit_value((unsigned char)*p);
        if (d >= base) break;
        if (acc > (ULLONG_MAX - (unsigned long long)d) / (unsigned long long)base) *overflow = 1;
        else acc = acc * (unsigned long long)base + (unsigned long long)d;
    }
    if (p == digits) { if (end) *end = (char *)s; return 0; }
    if (end) *end = (char *)p;
    return acc;
}

static long long signed_result(const char *s, char **end, int base, int c23) {
    int negative, overflow;
    unsigned long long acc = scan(s, end, base, c23, &negative, &overflow);
    unsigned long long limit = negative ? (unsigned long long)LLONG_MAX + 1u : (unsigned long long)LLONG_MAX;
    if (overflow || acc > limit) { fs_errno = FS_ERANGE; return negative ? LLONG_MIN : LLONG_MAX; }
    return negative ? (long long)(0ull - acc) : (long long)acc;
}

static unsigned long long unsigned_result(const char *s, char **end, int base, int c23) {
    int negative, overflow;
    unsigned long long acc = scan(s, end, base, c23, &negative, &overflow);
    if (overflow) { fs_errno = FS_ERANGE; return ULLONG_MAX; }
    return negative ? 0ull - acc : acc;   /* "-1" wraps, as in glibc; no ERANGE */
}

long long FS_NAME(strtoll)(const char *s, char **end, int base) { return signed_result(s, end, base, 0); }
unsigned long long FS_NAME(strtoull)(const char *s, char **end, int base) { return unsigned_result(s, end, base, 0); }
long FS_NAME(strtol)(const char *s, char **end, int base) { return (long)signed_result(s, end, base, 0); }
unsigned long FS_NAME(strtoul)(const char *s, char **end, int base) { return (unsigned long)unsigned_result(s, end, base, 0); }
long long FS_NAME(__isoc23_strtoll)(const char *s, char **end, int base) { return signed_result(s, end, base, 1); }
unsigned long long FS_NAME(__isoc23_strtoull)(const char *s, char **end, int base) { return unsigned_result(s, end, base, 1); }
long FS_NAME(__isoc23_strtol)(const char *s, char **end, int base) { return (long)signed_result(s, end, base, 1); }
unsigned long FS_NAME(__isoc23_strtoul)(const char *s, char **end, int base) { return (unsigned long)unsigned_result(s, end, base, 1); }

int FS_NAME(atoi)(const char *s) { return (int)FS_NAME(strtol)(s, NULL, 10); }
long FS_NAME(atol)(const char *s) { return FS_NAME(strtol)(s, NULL, 10); }
long long FS_NAME(atoll)(const char *s) { return FS_NAME(strtoll)(s, NULL, 10); }
