/* sscanf subset: conversions d i u o x X p s c n %, the * suppression, a
 * width, and length modifiers hh h l ll z t j. Not supported: floating
 * conversions, %[ scan sets, and positional arguments; the first such
 * specifier ends the scan with the count so far. The __isoc23_* names differ
 * only in that %i accepts a "0b" prefix, as with glibc 2.38+.
 */
#include "freestanding.h"
#include <limits.h>

static int is_space(int c) { return c == ' ' || (c >= '\t' && c <= '\r'); }

static int digit_value(int c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return 99;
}

/* Reads an integer of at most width bytes (0 = unlimited). Returns the number
 * of bytes consumed, 0 on matching failure. Overflow clamps like strtoll. */
static size_t scan_integer(const char *p, size_t width, int base, int is_signed, int c23, unsigned long long *out) {
    size_t i = 0, limit = width ? width : SIZE_MAX;
    int negative = 0, overflow = 0;
    if (i < limit && (p[i] == '+' || p[i] == '-')) { negative = p[i] == '-'; ++i; }
    if ((base == 0 || base == 16) && i + 2 < limit && p[i] == '0' && (p[i + 1] == 'x' || p[i + 1] == 'X')
        && digit_value((unsigned char)p[i + 2]) < 16) { i += 2; base = 16; }
    else if (c23 && (base == 0 || base == 2) && i + 2 < limit && p[i] == '0' && (p[i + 1] == 'b' || p[i + 1] == 'B')
        && digit_value((unsigned char)p[i + 2]) < 2) { i += 2; base = 2; }
    else if (base == 0) base = p[i] == '0' ? 8 : 10;
    unsigned long long acc = 0;
    size_t start = i;
    for (; i < limit; ++i) {
        int d = digit_value((unsigned char)p[i]);
        if (d >= base) break;
        if (acc > (ULLONG_MAX - (unsigned long long)d) / (unsigned long long)base) overflow = 1;
        else acc = acc * (unsigned long long)base + (unsigned long long)d;
    }
    if (i == start) return 0;
    if (is_signed) {
        unsigned long long lim = negative ? (unsigned long long)LLONG_MAX + 1u : (unsigned long long)LLONG_MAX;
        if (overflow || acc > lim) acc = negative ? (unsigned long long)LLONG_MIN : (unsigned long long)LLONG_MAX;
        else if (negative) acc = 0ull - acc;
    } else {
        if (overflow) acc = ULLONG_MAX;
        else if (negative) acc = 0ull - acc;
    }
    *out = acc;
    return i;
}

static void store(va_list *ap, const char *length, unsigned long long v) {
    if (length[0] == 'l' || length[0] == 'q') { if (length[1]) *va_arg(*ap, long long *) = (long long)v; else *va_arg(*ap, long *) = (long)v; }
    else if (length[0] == 'h') { if (length[1]) *va_arg(*ap, signed char *) = (signed char)v; else *va_arg(*ap, short *) = (short)v; }
    else if (length[0] == 'z') *va_arg(*ap, size_t *) = (size_t)v;
    else if (length[0] == 't') *va_arg(*ap, ptrdiff_t *) = (ptrdiff_t)v;
    else if (length[0] == 'j') *va_arg(*ap, intmax_t *) = (intmax_t)v;
    else *va_arg(*ap, int *) = (int)v;
}

static int scan(const char *input, const char *fmt, va_list ap, int c23) {
    const char *p = input;
    int count = 0;
    va_list args; va_copy(args, ap);
    for (; *fmt; ) {
        if (is_space((unsigned char)*fmt)) {
            while (is_space((unsigned char)*p)) ++p;
            while (is_space((unsigned char)*fmt)) ++fmt;
            continue;
        }
        if (*fmt != '%') {
            if (!*p) goto input_failure;
            if (*p != *fmt) goto done;
            ++p; ++fmt; continue;
        }
        ++fmt;
        int suppress = 0; size_t width = 0; char length[2] = {0, 0};
        if (*fmt == '*') { suppress = 1; ++fmt; }
        while (*fmt >= '0' && *fmt <= '9') { width = width * 10 + (size_t)(*fmt - '0'); ++fmt; }
        if (*fmt == 'h' || *fmt == 'l' || *fmt == 'z' || *fmt == 't' || *fmt == 'j' || *fmt == 'q') {
            length[0] = *fmt++;
            if ((length[0] == 'h' || length[0] == 'l') && *fmt == length[0]) length[1] = *fmt++;
        }
        char conv = *fmt ? *fmt++ : 0;
        switch (conv) {
        case '%':
            while (is_space((unsigned char)*p)) ++p;
            if (!*p) goto input_failure;
            if (*p != '%') goto done;
            ++p; break;
        case 'd': case 'i': case 'u': case 'o': case 'x': case 'X': case 'p': {
            while (is_space((unsigned char)*p)) ++p;
            if (!*p) goto input_failure;
            int base = conv == 'i' ? 0 : conv == 'u' || conv == 'd' ? 10 : conv == 'o' ? 8 : 16;
            unsigned long long v;
            size_t n = scan_integer(p, width, base, conv == 'd' || conv == 'i', c23, &v);
            if (!n) goto done;
            p += n;
            if (!suppress) {
                if (conv == 'p') *va_arg(args, void **) = (void *)(uintptr_t)v; else store(&args, length, v);
                ++count;
            }
            break;
        }
        case 's': {
            while (is_space((unsigned char)*p)) ++p;
            if (!*p) goto input_failure;
            char *out = suppress ? NULL : va_arg(args, char *);
            size_t i = 0, limit = width ? width : SIZE_MAX;
            for (; i < limit && p[i] && !is_space((unsigned char)p[i]); ++i) if (out) out[i] = p[i];
            if (out) { out[i] = 0; ++count; }
            p += i;
            break;
        }
        case 'c': {
            /* glibc stores a short read at end of input and counts it. */
            size_t n = FS_NAME(strnlen)(p, width ? width : 1);
            if (!n) goto input_failure;
            if (!suppress) { memcpy(va_arg(args, char *), p, n); ++count; }
            p += n;
            break;
        }
        case 'n':
            if (!suppress) store(&args, length, (unsigned long long)(p - input));
            break;
        default:
            goto done;   /* unsupported conversion or dangling '%' */
        }
    }
done:
    va_end(args);
    return count;
input_failure:
    va_end(args);
    return count ? count : -1;
}

int FS_NAME(vsscanf)(const char *s, const char *fmt, va_list ap) { return scan(s, fmt, ap, 0); }
int FS_NAME(__isoc99_vsscanf)(const char *s, const char *fmt, va_list ap) { return scan(s, fmt, ap, 0); }
int FS_NAME(__isoc23_vsscanf)(const char *s, const char *fmt, va_list ap) { return scan(s, fmt, ap, 1); }

int FS_NAME(sscanf)(const char *s, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int r = scan(s, fmt, ap, 0);
    va_end(ap); return r;
}
int FS_NAME(__isoc99_sscanf)(const char *s, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int r = scan(s, fmt, ap, 0);
    va_end(ap); return r;
}
int FS_NAME(__isoc23_sscanf)(const char *s, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int r = scan(s, fmt, ap, 1);
    va_end(ap); return r;
}
