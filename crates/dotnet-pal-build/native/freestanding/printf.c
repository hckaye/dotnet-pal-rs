/* vsnprintf family. Supported: flags "-+ 0#", width and precision (numeric
 * or *), length modifiers hh h l ll z t j L, conversions d i u x X o c s p n %
 * and f F e E g G. Floating output is an exact binary-to-decimal expansion,
 * rounded half-to-even on the exact value (glibc's default-mode result), so
 * digits match glibc; the two limits are that at most FS_MAX_DIGITS fractional
 * or significant digits are computed (later positions print as '0') and that
 * %L values are narrowed to double before formatting. No locale, no %a, no
 * positional (%1$) arguments.
 */
#include "freestanding.h"
#include <limits.h>

#define FS_MAX_DIGITS 512

typedef struct { char *buf; size_t cap; size_t len; } sink;

static void put(sink *s, char c) {
    if (s->len + 1 < s->cap) s->buf[s->len] = c;
    s->len++;
}
static void put_n(sink *s, const char *p, size_t n) { for (size_t i = 0; i < n; ++i) put(s, p[i]); }
static void pad(sink *s, char c, size_t n) { while (n--) put(s, c); }

typedef struct {
    int left, plus, space, alt, zero;
    int width;   /* -1 when absent */
    int prec;    /* -1 when absent */
    char length[2];
} spec;

/* Writes prefix and left padding for a field whose body is blen bytes; returns
 * the number of trailing spaces the caller must emit after the body. */
static size_t begin_field(sink *s, const spec *sp, const char *prefix, size_t plen, size_t blen, int zero_ok) {
    size_t total = plen + blen;
    size_t padn = sp->width > 0 && (size_t)sp->width > total ? (size_t)sp->width - total : 0;
    if (sp->left) { put_n(s, prefix, plen); return padn; }
    if (sp->zero && zero_ok) { put_n(s, prefix, plen); pad(s, '0', padn); }
    else { pad(s, ' ', padn); put_n(s, prefix, plen); }
    return 0;
}

static void fmt_integer(sink *s, const spec *sp, unsigned long long mag, int negative, int is_signed, int base, int upper) {
    char digits[24];
    size_t n = 0;
    const char *alphabet = upper ? "0123456789ABCDEF" : "0123456789abcdef";
    while (mag) { digits[sizeof digits - ++n] = alphabet[mag % (unsigned)base]; mag /= (unsigned)base; }
    if (n == 0 && sp->prec != 0) digits[sizeof digits - ++n] = '0';
    size_t zeros = sp->prec > 0 && (size_t)sp->prec > n ? (size_t)sp->prec - n : 0;
    char prefix[3]; size_t plen = 0;
    if (is_signed) {
        if (negative) prefix[plen++] = '-';
        else if (sp->plus) prefix[plen++] = '+';
        else if (sp->space) prefix[plen++] = ' ';
    }
    if (sp->alt && base == 16 && n && digits[sizeof digits - n] != '0') { prefix[plen++] = '0'; prefix[plen++] = upper ? 'X' : 'x'; }
    if (sp->alt && base == 8 && zeros == 0 && (n == 0 || digits[sizeof digits - n] != '0')) zeros = 1;
    size_t trailing = begin_field(s, sp, prefix, plen, zeros + n, sp->prec < 0);
    pad(s, '0', zeros);
    put_n(s, digits + sizeof digits - n, n);
    pad(s, ' ', trailing);
}

/* --- exact decimal expansion of a finite double ------------------------- */

/* Fractional part R / 2^s as a little-endian binary bignum; s == 0 means no
 * fractional part. Digits are pulled out by multiplying by ten. */
typedef struct { uint32_t w[36]; int s; } fraction;

static void fraction_init(fraction *f, uint64_t r, int s) {
    for (size_t i = 0; i < sizeof f->w / sizeof f->w[0]; ++i) f->w[i] = 0;
    f->w[0] = (uint32_t)r; f->w[1] = (uint32_t)(r >> 32);
    f->s = s;
}

static int fraction_digit(fraction *f) {
    if (f->s == 0) return 0;
    int idx = f->s / 32, sh = f->s % 32;
    uint64_t carry = 0;
    for (int i = 0; i <= idx + 1; ++i) {
        uint64_t v = (uint64_t)f->w[i] * 10u + carry;
        f->w[i] = (uint32_t)v; carry = v >> 32;
    }
    uint64_t top = (((uint64_t)f->w[idx + 1] << 32) | f->w[idx]) >> sh;
    f->w[idx] &= (1u << sh) - 1u;
    f->w[idx + 1] = 0;
    return (int)top;
}

/* -1, 0, 1 as the remaining fraction compares with one half. */
static int fraction_cmp_half(const fraction *f) {
    if (f->s == 0) return -1;
    int bit = f->s - 1, idx = bit / 32, sh = bit % 32;
    if (!((f->w[idx] >> sh) & 1u)) return -1;
    if (f->w[idx] & ((1u << sh) - 1u)) return 1;
    for (int i = 0; i < idx; ++i) if (f->w[i]) return 1;
    return 0;
}

static int fraction_is_zero(const fraction *f) {
    if (f->s == 0) return 1;
    for (int i = 0; i <= f->s / 32 + 1; ++i) if (f->w[i]) return 0;
    return 1;
}

/* Decimal digits of m * 2^e (e >= 0) into out; returns the count (0 for zero). */
static int integer_digits(uint64_t m, int e, char *out) {
    uint32_t limb[36]; int n = 0;
    while (m) { limb[n++] = (uint32_t)(m % 1000000000u); m /= 1000000000u; }
    while (e > 0 && n) {
        int sh = e > 28 ? 28 : e; e -= sh;
        uint64_t carry = 0;
        for (int i = 0; i < n; ++i) {
            uint64_t v = ((uint64_t)limb[i] << sh) + carry;
            limb[i] = (uint32_t)(v % 1000000000u); carry = v / 1000000000u;
        }
        if (carry) limb[n++] = (uint32_t)carry;
    }
    int len = 0;
    for (int i = n - 1; i >= 0; --i) {
        char tmp[9]; uint32_t v = limb[i];
        for (int k = 8; k >= 0; --k) { tmp[k] = (char)('0' + v % 10u); v /= 10u; }
        int start = 0;
        if (i == n - 1) while (start < 8 && tmp[start] == '0') ++start;
        for (int k = start; k < 9; ++k) out[len++] = tmp[k];
    }
    return len;
}

typedef struct {
    char digits[FS_MAX_DIGITS + 340];  /* integer digits then fractional digits */
    int ilen;                          /* integer digits (>= 1 after generation) */
    fraction frac;
} decimal;

static void decimal_init(decimal *d, uint64_t m, int e2) {
    if (e2 >= 0) { d->ilen = integer_digits(m, e2, d->digits); fraction_init(&d->frac, 0, 0); }
    else {
        int s = -e2;
        uint64_t ip = s < 64 ? m >> s : 0, r = s < 64 ? m & ((1ull << s) - 1u) : m;
        d->ilen = integer_digits(ip, 0, d->digits);
        fraction_init(&d->frac, r, s);
    }
}

/* Propagates a +1 into digits[0..n); returns 1 when a new leading digit appeared. */
static int round_up(char *digits, int n) {
    for (int i = n - 1; i >= 0; --i) {
        if (digits[i] != '9') { digits[i]++; return 0; }
        digits[i] = '0';
    }
    for (int i = n; i > 0; --i) digits[i] = digits[i - 1];
    digits[0] = '1';
    return 1;
}

/* %f digits: integer part followed by prec fractional digits, rounded. */
static void gen_fixed(decimal *d, int prec) {
    if (d->ilen == 0) { d->digits[0] = '0'; d->ilen = 1; }
    for (int i = 0; i < prec; ++i) d->digits[d->ilen + i] = (char)('0' + fraction_digit(&d->frac));
    int n = d->ilen + prec, cmp = fraction_cmp_half(&d->frac);
    if (cmp > 0 || (cmp == 0 && ((d->digits[n - 1] - '0') & 1))) d->ilen += round_up(d->digits, n);
}

/* %e digits: nsig significant digits into sig, rounded; returns the decimal exponent. */
static int gen_exp(decimal *d, int nsig, char *sig) {
    int exp10, cmp;
    if (d->ilen == 0) {
        if (fraction_is_zero(&d->frac)) { for (int i = 0; i < nsig; ++i) sig[i] = '0'; return 0; }
        int lead = 0, digit;
        while ((digit = fraction_digit(&d->frac)) == 0) ++lead;
        exp10 = -(lead + 1);
        sig[0] = (char)('0' + digit);
        for (int i = 1; i < nsig; ++i) sig[i] = (char)('0' + fraction_digit(&d->frac));
        cmp = fraction_cmp_half(&d->frac);
    } else {
        exp10 = d->ilen - 1;
        if (nsig <= d->ilen) {
            memcpy(sig, d->digits, (size_t)nsig);
            if (nsig == d->ilen) cmp = fraction_cmp_half(&d->frac);
            else {
                char c = d->digits[nsig];
                if (c > '5') cmp = 1;
                else if (c < '5') cmp = -1;
                else {
                    cmp = fraction_is_zero(&d->frac) ? 0 : 1;
                    for (int i = nsig + 1; i < d->ilen; ++i) if (d->digits[i] != '0') cmp = 1;
                }
            }
        } else {
            memcpy(sig, d->digits, (size_t)d->ilen);
            for (int i = d->ilen; i < nsig; ++i) sig[i] = (char)('0' + fraction_digit(&d->frac));
            cmp = fraction_cmp_half(&d->frac);
        }
    }
    if (cmp > 0 || (cmp == 0 && ((sig[nsig - 1] - '0') & 1))) {
        if (round_up(sig, nsig)) { sig[nsig] = '0'; ++exp10; }  /* 999.. -> 1000..: drop the extra digit */
    }
    return exp10;
}

static void put_exponent(sink *s, int exp10, int upper) {
    put(s, upper ? 'E' : 'e');
    put(s, exp10 < 0 ? '-' : '+');
    unsigned e = exp10 < 0 ? (unsigned)-exp10 : (unsigned)exp10;
    char tmp[4]; int n = 0;
    do { tmp[n++] = (char)('0' + e % 10u); e /= 10u; } while (e);
    if (n < 2) tmp[n++] = '0';
    while (n) put(s, tmp[--n]);
}

static void fmt_float(sink *s, const spec *sp, double v, char conv) {
    int upper = conv >= 'A' && conv <= 'Z';
    if (upper) conv = (char)(conv + ('a' - 'A'));
    uint64_t bits; memcpy(&bits, &v, sizeof bits);
    int negative = (int)(bits >> 63), exp = (int)((bits >> 52) & 0x7ff);
    uint64_t man = bits & ((1ull << 52) - 1u);
    char prefix[1]; size_t plen = 0;
    if (negative) prefix[plen++] = '-'; else if (sp->plus) prefix[plen++] = '+'; else if (sp->space) prefix[plen++] = ' ';
    if (exp == 0x7ff) {
        const char *body = man ? (upper ? "NAN" : "nan") : (upper ? "INF" : "inf");
        size_t trailing = begin_field(s, sp, prefix, plen, 3, 0);
        put_n(s, body, 3);
        pad(s, ' ', trailing);
        return;
    }
    uint64_t m = exp ? man | (1ull << 52) : man;
    int e2 = exp ? exp - 1075 : -1074;
    int prec = sp->prec < 0 ? 6 : sp->prec;
    decimal d;
    decimal_init(&d, m, e2);
    if (conv == 'g') {
        int p = prec == 0 ? 1 : prec;
        int nsig = p > FS_MAX_DIGITS ? FS_MAX_DIGITS : p;
        decimal probe = d;
        char sig[FS_MAX_DIGITS + 1];
        int x = gen_exp(&probe, nsig, sig);
        if (x < -4 || x >= p) {
            int frac_digits = p - 1;
            int shown = frac_digits > FS_MAX_DIGITS - 1 ? FS_MAX_DIGITS - 1 : frac_digits;
            if (!sp->alt) while (shown > 0 && sig[shown] == '0') --shown;
            int point = shown > 0 || sp->alt;
            size_t blen = 1u + (size_t)point + (size_t)shown + (sp->alt ? (size_t)(frac_digits - shown) : 0u) + 2u + (size_t)(x >= 100 || x <= -100 ? 3 : 2);
            size_t trailing = begin_field(s, sp, prefix, plen, blen, 1);
            put(s, sig[0]);
            if (point) put(s, '.');
            put_n(s, sig + 1, (size_t)shown);
            if (sp->alt) pad(s, '0', (size_t)(frac_digits - shown));
            put_exponent(s, x, upper);
            pad(s, ' ', trailing);
        } else {
            int frac_digits = p - 1 - x;
            int gen = frac_digits > FS_MAX_DIGITS ? FS_MAX_DIGITS : frac_digits;
            gen_fixed(&d, gen);
            int shown = gen, zeros = sp->alt ? frac_digits - gen : 0;
            if (!sp->alt) while (shown > 0 && d.digits[d.ilen + shown - 1] == '0') --shown;
            int point = shown > 0 || sp->alt;
            size_t blen = (size_t)d.ilen + (size_t)point + (size_t)shown + (size_t)zeros;
            size_t trailing = begin_field(s, sp, prefix, plen, blen, 1);
            put_n(s, d.digits, (size_t)d.ilen);
            if (point) put(s, '.');
            put_n(s, d.digits + d.ilen, (size_t)shown);
            pad(s, '0', (size_t)zeros);
            pad(s, ' ', trailing);
        }
        return;
    }
    if (conv == 'e') {
        int nsig = prec + 1 > FS_MAX_DIGITS ? FS_MAX_DIGITS : prec + 1;
        char sig[FS_MAX_DIGITS + 1];
        int x = gen_exp(&d, nsig, sig);
        int point = prec > 0 || sp->alt;
        size_t blen = 1u + (size_t)point + (size_t)prec + 2u + (size_t)(x >= 100 || x <= -100 ? 3 : 2);
        size_t trailing = begin_field(s, sp, prefix, plen, blen, 1);
        put(s, sig[0]);
        if (point) put(s, '.');
        put_n(s, sig + 1, (size_t)(nsig - 1));
        pad(s, '0', (size_t)(prec + 1 - nsig));
        put_exponent(s, x, upper);
        pad(s, ' ', trailing);
        return;
    }
    int shown = prec > FS_MAX_DIGITS ? FS_MAX_DIGITS : prec;
    gen_fixed(&d, shown);
    int point = prec > 0 || sp->alt;
    size_t blen = (size_t)d.ilen + (size_t)point + (size_t)prec;
    size_t trailing = begin_field(s, sp, prefix, plen, blen, 1);
    put_n(s, d.digits, (size_t)d.ilen);
    if (point) put(s, '.');
    put_n(s, d.digits + d.ilen, (size_t)shown);
    pad(s, '0', (size_t)(prec - shown));
    pad(s, ' ', trailing);
}

/* --- driver --------------------------------------------------------------- */

static int parse_number(const char **p) {
    int v = 0;
    while (**p >= '0' && **p <= '9') { if (v < INT_MAX / 10) v = v * 10 + (**p - '0'); ++*p; }
    return v;
}

static void format(sink *s, const char *fmt, va_list ap) {
    for (const char *p = fmt; *p; ++p) {
        if (*p != '%') { put(s, *p); continue; }
        ++p;
        spec sp = {0, 0, 0, 0, 0, -1, -1, {0, 0}};
        for (;; ++p) {
            if (*p == '-') sp.left = 1; else if (*p == '+') sp.plus = 1; else if (*p == ' ') sp.space = 1;
            else if (*p == '#') sp.alt = 1; else if (*p == '0') sp.zero = 1; else break;
        }
        if (*p == '*') { sp.width = va_arg(ap, int); if (sp.width < 0) { sp.left = 1; sp.width = sp.width == INT_MIN ? INT_MAX : -sp.width; } ++p; }
        else if (*p >= '1' && *p <= '9') sp.width = parse_number(&p);
        if (*p == '.') {
            ++p;
            if (*p == '*') { sp.prec = va_arg(ap, int); if (sp.prec < 0) sp.prec = -1; ++p; }
            else sp.prec = parse_number(&p);
        }
        if (*p == 'h' || *p == 'l' || *p == 'z' || *p == 't' || *p == 'j' || *p == 'L' || *p == 'q') {
            sp.length[0] = *p++;
            if ((sp.length[0] == 'h' || sp.length[0] == 'l') && *p == sp.length[0]) sp.length[1] = *p++;
        }
        char conv = *p;
        switch (conv) {
        case 'd': case 'i': {
            long long v;
            if (sp.length[0] == 'l' || sp.length[0] == 'q') v = sp.length[1] ? va_arg(ap, long long) : va_arg(ap, long);
            else if (sp.length[0] == 'h') v = sp.length[1] ? (signed char)va_arg(ap, int) : (short)va_arg(ap, int);
            else if (sp.length[0] == 'z') v = (long long)va_arg(ap, size_t);
            else if (sp.length[0] == 't') v = va_arg(ap, ptrdiff_t);
            else if (sp.length[0] == 'j') v = (long long)va_arg(ap, intmax_t);
            else v = va_arg(ap, int);
            fmt_integer(s, &sp, v < 0 ? 0ull - (unsigned long long)v : (unsigned long long)v, v < 0, 1, 10, 0);
            break;
        }
        case 'u': case 'x': case 'X': case 'o': {
            unsigned long long v;
            if (sp.length[0] == 'l' || sp.length[0] == 'q') v = sp.length[1] ? va_arg(ap, unsigned long long) : va_arg(ap, unsigned long);
            else if (sp.length[0] == 'h') v = sp.length[1] ? (unsigned char)va_arg(ap, unsigned) : (unsigned short)va_arg(ap, unsigned);
            else if (sp.length[0] == 'z') v = va_arg(ap, size_t);
            else if (sp.length[0] == 't') v = (unsigned long long)va_arg(ap, ptrdiff_t);
            else if (sp.length[0] == 'j') v = (unsigned long long)va_arg(ap, uintmax_t);
            else v = va_arg(ap, unsigned);
            fmt_integer(s, &sp, v, 0, 0, conv == 'u' ? 10 : conv == 'o' ? 8 : 16, conv == 'X');
            break;
        }
        case 'p': {
            void *v = va_arg(ap, void *);
            if (!v) { size_t t = begin_field(s, &sp, "", 0, 5, 0); put_n(s, "(nil)", 5); pad(s, ' ', t); }
            else { sp.alt = 1; fmt_integer(s, &sp, (uintptr_t)v, 0, 0, 16, 0); }
            break;
        }
        case 'c': {
            char c = (char)va_arg(ap, int);
            size_t t = begin_field(s, &sp, "", 0, 1, 0);
            put(s, c); pad(s, ' ', t);
            break;
        }
        case 's': {
            const char *v = va_arg(ap, const char *);
            size_t n;
            if (!v) { v = "(null)"; n = sp.prec >= 0 && sp.prec < 6 ? 0 : 6; }
            else n = sp.prec >= 0 ? FS_NAME(strnlen)(v, (size_t)sp.prec) : FS_NAME(strlen)(v);
            size_t t = begin_field(s, &sp, "", 0, n, 0);
            put_n(s, v, n); pad(s, ' ', t);
            break;
        }
        case 'f': case 'F': case 'e': case 'E': case 'g': case 'G': {
            double v = sp.length[0] == 'L' ? (double)va_arg(ap, long double) : va_arg(ap, double);
            fmt_float(s, &sp, v, conv);
            break;
        }
        case 'n': {
            size_t n = s->len;
            if (sp.length[0] == 'l') { if (sp.length[1]) *va_arg(ap, long long *) = (long long)n; else *va_arg(ap, long *) = (long)n; }
            else if (sp.length[0] == 'h') { if (sp.length[1]) *va_arg(ap, signed char *) = (signed char)n; else *va_arg(ap, short *) = (short)n; }
            else if (sp.length[0] == 'z') *va_arg(ap, size_t *) = n;
            else if (sp.length[0] == 't') *va_arg(ap, ptrdiff_t *) = (ptrdiff_t)n;
            else if (sp.length[0] == 'j') *va_arg(ap, intmax_t *) = (intmax_t)n;
            else *va_arg(ap, int *) = (int)n;
            break;
        }
        case '%': put(s, '%'); break;
        case 0: return;                     /* dangling '%': glibc prints nothing */
        default: put(s, '%'); put(s, conv); break;
        }
    }
}

int FS_NAME(vsnprintf)(char *buf, size_t n, const char *fmt, va_list ap) {
    sink s = {buf, n, 0};
    format(&s, fmt, ap);
    if (n) buf[s.len < n ? s.len : n - 1] = 0;
    return s.len > INT_MAX ? -1 : (int)s.len;
}

int FS_NAME(snprintf)(char *buf, size_t n, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int r = FS_NAME(vsnprintf)(buf, n, fmt, ap);
    va_end(ap);
    return r;
}

int FS_NAME(vsprintf)(char *buf, const char *fmt, va_list ap) { return FS_NAME(vsnprintf)(buf, SIZE_MAX, fmt, ap); }

int FS_NAME(sprintf)(char *buf, const char *fmt, ...) {
    va_list ap; va_start(ap, fmt);
    int r = FS_NAME(vsnprintf)(buf, SIZE_MAX, fmt, ap);
    va_end(ap);
    return r;
}
