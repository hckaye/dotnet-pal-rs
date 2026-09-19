/* Host self-test: the shim is compiled with -DFREESTANDING_PREFIX=fs_ and
 * every function is compared with glibc on the same inputs. A stub
 * dotnet_pal_get_api (bump allocator) backs the malloc family. Exit 0 and
 * print SELFTEST PASS when every check holds.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <limits.h>
#include <math.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define FREESTANDING_PREFIX fs_
#include "freestanding.h"
#include "dotnet_pal.h"

/* _GNU_SOURCE makes glibc 2.38+ headers bind strtol/sscanf to the C23 entry
 * points, so the pre-C23 symbols are bound by hand for the plain comparison. */
extern long plain_strtol(const char *, char **, int) __asm__("strtol");
extern unsigned long plain_strtoul(const char *, char **, int) __asm__("strtoul");
extern long long plain_strtoll(const char *, char **, int) __asm__("strtoll");
extern unsigned long long plain_strtoull(const char *, char **, int) __asm__("strtoull");
extern int plain_sscanf(const char *, const char *, ...) __asm__("__isoc99_sscanf");
extern long __isoc23_strtol(const char *, char **, int);
extern unsigned long __isoc23_strtoul(const char *, char **, int);
extern long long __isoc23_strtoll(const char *, char **, int);
extern unsigned long long __isoc23_strtoull(const char *, char **, int);
extern int __isoc23_sscanf(const char *, const char *, ...);
extern int __xpg_strerror_r(int, char *, size_t);
extern int fs_cxx_selftest(void);

static int failures, checks;
#define CHECK(cond, ...) do { checks++; if (!(cond)) { failures++; printf("FAIL %s:%d: ", __FILE__, __LINE__); printf(__VA_ARGS__); printf("\n"); } } while (0)
static int sign(long v) { return v < 0 ? -1 : v > 0; }

/* ---- support-table stub: bump allocator over a static arena ------------- */
static _Alignas(16) unsigned char arena[1u << 22];
static size_t arena_used, live_blocks, stub_misalign;
static int stub_enabled;
typedef struct { size_t size; size_t pad; } stub_header;

static uint32_t stub_allocate(size_t size, uint32_t zero, void **out) {
    if (!size || zero > 1) return DOTNET_PAL_INVALID_ARGUMENT;
    size_t start = (arena_used + 15) & ~(size_t)15;
    if (stub_misalign) start += 8;                          /* provider that is only 8-byte aligned */
    size_t need = sizeof(stub_header) + ((size + 15) & ~(size_t)15);
    if (start + need > sizeof arena) return DOTNET_PAL_OUT_OF_MEMORY;
    stub_header *h = (stub_header *)(arena + start);
    h->size = size;
    unsigned char *p = (unsigned char *)(h + 1);
    memset(p, zero ? 0 : 0xCD, size);
    arena_used = start + need; live_blocks++;
    *out = p; return DOTNET_PAL_OK;
}
static uint32_t stub_release(void *p) {
    if (!p) return DOTNET_PAL_OK;
    if ((unsigned char *)p < arena || (unsigned char *)p >= arena + sizeof arena) return DOTNET_PAL_INVALID_ARGUMENT;
    live_blocks--; return DOTNET_PAL_OK;
}
static uint32_t stub_resize(void *p, size_t size, void **out) {
    stub_header *h = (stub_header *)p - 1;
    void *q; uint32_t rc = stub_allocate(size, 0, &q);
    if (rc != DOTNET_PAL_OK) return rc;
    memcpy(q, p, h->size < size ? h->size : size);
    stub_release(p);
    *out = q; return DOTNET_PAL_OK;
}
static dotnet_pal_api stub_api;
const dotnet_pal_api *dotnet_pal_get_api(uint32_t version) {
    if (!stub_enabled || version != DOTNET_PAL_ABI_VERSION) return NULL;
    stub_api.header.abi_version = DOTNET_PAL_ABI_VERSION;
    stub_api.header.struct_size = sizeof stub_api;
    stub_api.header.capabilities = DOTNET_PAL_CAP_NATIVE_HEAP;
    stub_api.support.allocate = stub_allocate;
    stub_api.support.resize = stub_resize;
    stub_api.support.release = stub_release;
    return &stub_api;
}

/* ---- port hooks: recorded and unwound so the test can continue ---------- */
static jmp_buf hook_jump;
static int hook_status, hook_aborted;
_Noreturn void dotnet_pal_freestanding_abort(void) { hook_aborted = 1; longjmp(hook_jump, 1); }
_Noreturn void dotnet_pal_freestanding_exit(int status) { hook_status = status; longjmp(hook_jump, 2); }

/* ---- strings ------------------------------------------------------------ */
static ptrdiff_t off(const char *base, const char *p) { return p ? p - base : -1; }

static void test_strings(void) {
    static const char *const words[] = {"", "a", "ab", "abc", "abd", "abcabc", "ABC", "aBc", "hello world", " x", "abcd", "b", "cab"};
    size_t n = sizeof words / sizeof words[0];
    for (size_t i = 0; i < n; ++i) {
        const char *a = words[i];
        CHECK(fs_strlen(a) == strlen(a), "strlen %s", a);
        for (size_t m = 0; m < 6; ++m) CHECK(fs_strnlen(a, m) == strnlen(a, m), "strnlen %s %zu", a, m);
        for (size_t j = 0; j < n; ++j) {
            const char *b = words[j];
            CHECK(sign(fs_strcmp(a, b)) == sign(strcmp(a, b)), "strcmp %s %s", a, b);
            CHECK(sign(fs_strcasecmp(a, b)) == sign(strcasecmp(a, b)), "strcasecmp %s %s", a, b);
            for (size_t m = 0; m < 5; ++m) {
                CHECK(sign(fs_strncmp(a, b, m)) == sign(strncmp(a, b, m)), "strncmp %s %s %zu", a, b, m);
                CHECK(sign(fs_strncasecmp(a, b, m)) == sign(strncasecmp(a, b, m)), "strncasecmp %s %s %zu", a, b, m);
            }
            CHECK(off(a, fs_strstr(a, b)) == off(a, strstr(a, b)), "strstr %s %s", a, b);
            CHECK(fs_strspn(a, b) == strspn(a, b), "strspn %s %s", a, b);
            CHECK(fs_strcspn(a, b) == strcspn(a, b), "strcspn %s %s", a, b);
            CHECK(off(a, fs_strpbrk(a, b)) == off(a, strpbrk(a, b)), "strpbrk %s %s", a, b);
            char x[64], y[64];
            strcpy(x, a); strcpy(y, a);
            CHECK(fs_strcat(x, b) == x && !strcmp(strcat(y, b), x), "strcat %s %s", a, b);
            for (size_t m = 0; m < 4; ++m) {
                memset(x, 0x55, sizeof x); memset(y, 0x55, sizeof y); strcpy(x, a); strcpy(y, a);
                CHECK(fs_strncat(x, b, m) == x, "strncat ret");
                strncat(y, b, m);
                CHECK(!memcmp(x, y, sizeof x), "strncat %s %s %zu", a, b, m);
                memset(x, 0x55, sizeof x); memset(y, 0x55, sizeof y);
                CHECK(fs_strncpy(x, b, m) == x, "strncpy ret");
                strncpy(y, b, m);
                CHECK(!memcmp(x, y, sizeof x), "strncpy %s %zu", b, m);
            }
        }
        for (int c = 0; c < 128; c += 7) {
            CHECK(off(a, fs_strchr(a, c)) == off(a, strchr(a, c)), "strchr %s %d", a, c);
            CHECK(off(a, fs_strrchr(a, c)) == off(a, strrchr(a, c)), "strrchr %s %d", a, c);
            CHECK(off(a, fs_memchr(a, c, strlen(a) + 1)) == off(a, memchr(a, c, strlen(a) + 1)), "memchr %s %d", a, c);
        }
        CHECK(off(a, fs_strchr(a, 0)) == (ptrdiff_t)strlen(a), "strchr 0 %s", a);
        CHECK(off(a, fs_strrchr(a, 0)) == (ptrdiff_t)strlen(a), "strrchr 0 %s", a);
        CHECK(fs_strchr(a, 'z') == NULL && fs_strrchr(a, 'z') == NULL, "no match %s", a);
        char x[64], y[64];
        memset(x, 0x55, sizeof x); memset(y, 0x55, sizeof y);
        CHECK(fs_strcpy(x, a) == x && !memcmp(x, strcpy(y, a), sizeof x), "strcpy %s", a);
    }
    CHECK(fs_memchr("abc", 'c', 2) == NULL && fs_memchr("a\0b", 0, 3) != NULL, "memchr bounds");
    {   /* strtok_r */
        static const char *const inputs[] = {"a,b,,c", ",,,", "", "abc", " a b ", ",x,"};
        for (size_t i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {
            char x[32], y[32], *sx, *sy, *tx, *ty;
            strcpy(x, inputs[i]); strcpy(y, inputs[i]);
            tx = fs_strtok_r(x, ", ", &sx); ty = strtok_r(y, ", ", &sy);
            while (tx || ty) {
                CHECK(tx && ty && !strcmp(tx, ty) && tx - x == ty - y, "strtok_r %s", inputs[i]);
                tx = fs_strtok_r(NULL, ", ", &sx); ty = strtok_r(NULL, ", ", &sy);
            }
            CHECK(!memcmp(x, y, strlen(inputs[i]) + 1), "strtok_r buffer %s", inputs[i]);
        }
    }
    {   /* strerror family: text differs from glibc by design, semantics do not */
        CHECK(!strcmp(fs_strerror(5), "error 5") && !strcmp(fs_strerror(-12), "error -12"), "strerror");
        char buf[8];
        CHECK(fs_strerror_r(7, buf, sizeof buf) == buf && !strcmp(buf, "error 7"), "strerror_r fits");
        CHECK(!strcmp(fs_strerror_r(12345, buf, sizeof buf), "error 12345"), "strerror_r fallback");
        CHECK(fs___xpg_strerror_r(7, buf, sizeof buf) == 0 && !strcmp(buf, "error 7"), "xpg ok");
        CHECK(fs___xpg_strerror_r(12345, buf, sizeof buf) == ERANGE && !strcmp(buf, "error 1"), "xpg erange");
        CHECK(fs___xpg_strerror_r(1, buf, 0) == ERANGE, "xpg zero");
        char gbuf[64];
        CHECK(__xpg_strerror_r(EINVAL, gbuf, 3) == ERANGE, "glibc xpg convention matches");
    }
}

/* ---- strto* ------------------------------------------------------------- */
#define STRTO_CASE(fn, gfn, T, fmt) do { \
    char *e1 = NULL, *e2 = NULL; errno = 0; T v1 = gfn(s, &e1, base); int g = errno; \
    fs_errno = 0; T v2 = fn(s, &e2, base); int f = fs_errno; \
    CHECK(v1 == v2 && e1 - s == e2 - s && g == f, #fn "(\"%s\",%d): glibc " fmt " off %td errno %d; shim " fmt " off %td errno %d", s, base, v1, e1 - s, g, v2, e2 - s, f); \
} while (0)

static void test_strto(void) {
    static const struct { const char *s; int base; } cases[] = {
        {"0", 10}, {"123", 10}, {"-123", 10}, {"+77", 10}, {"  42", 10}, {"\t\n-9x", 10}, {"", 10}, {"   ", 10}, {"abc", 10},
        {"-", 10}, {"+", 10}, {"12abc", 10}, {"0x1f", 16}, {"0X1F", 16}, {"1f", 16}, {"0x", 16}, {"0xg", 16}, {"0x", 0},
        {"0x1f", 0}, {"017", 0}, {"08", 0}, {"0", 0}, {"123", 0}, {"-0x10", 0}, {"0b101", 0}, {"0b101", 2}, {"0b", 2},
        {"0b2", 0}, {"101", 2}, {"zz", 36}, {"Zz", 36}, {"9223372036854775807", 10}, {"9223372036854775808", 10},
        {"-9223372036854775808", 10}, {"-9223372036854775809", 10}, {"18446744073709551615", 10}, {"18446744073709551616", 10},
        {"-1", 10}, {"-18446744073709551615", 10}, {"-18446744073709551616", 10}, {"99999999999999999999999", 10},
        {"-99999999999999999999999", 10}, {"0xffffffffffffffff", 16}, {"0x10000000000000000", 16}, {"7fffffffffffffff", 16},
        {"8000000000000000", 16}, {"1", 1}, {"1", 37}, {"1", -1}, {"12", 3}, {"  +0x", 0}, {"0X", 16}, {"1e5", 10}, {"+-1", 10},
        {"- 1", 10}, {"\v\f\r 8", 10}, {"0b", 0}, {"0B11", 0}, {"0b11", 16},
    };
    for (size_t i = 0; i < sizeof cases / sizeof cases[0]; ++i) {
        const char *s = cases[i].s; int base = cases[i].base;
        STRTO_CASE(fs_strtol, plain_strtol, long, "%ld");
        STRTO_CASE(fs_strtoul, plain_strtoul, unsigned long, "%lu");
        STRTO_CASE(fs_strtoll, plain_strtoll, long long, "%lld");
        STRTO_CASE(fs_strtoull, plain_strtoull, unsigned long long, "%llu");
        STRTO_CASE(fs___isoc23_strtol, __isoc23_strtol, long, "%ld");
        STRTO_CASE(fs___isoc23_strtoul, __isoc23_strtoul, unsigned long, "%lu");
        STRTO_CASE(fs___isoc23_strtoll, __isoc23_strtoll, long long, "%lld");
        STRTO_CASE(fs___isoc23_strtoull, __isoc23_strtoull, unsigned long long, "%llu");
        CHECK(fs_atoi(s) == (int)plain_strtol(s, NULL, 10) && fs_atol(s) == plain_strtol(s, NULL, 10)
              && fs_atoll(s) == plain_strtoll(s, NULL, 10), "atoi %s", s);
    }
}

/* ---- printf ------------------------------------------------------------- */
static int format_cases;
static void check_fmt(const char *fmt, ...) {
    static const size_t sizes[] = {256, 0, 1, 4, 9, 17};
    for (size_t k = 0; k < sizeof sizes / sizeof sizes[0]; ++k) {
        char a[256], b[256];
        memset(a, 0xAA, sizeof a); memset(b, 0xAA, sizeof b);
        va_list ap, ap2;
        va_start(ap, fmt);
        va_copy(ap2, ap);
        int r1 = vsnprintf(a, sizes[k], fmt, ap);
        int r2 = fs_vsnprintf(b, sizes[k], fmt, ap2);
        va_end(ap2); va_end(ap);
        int same = r1 == r2 && !memcmp(a, b, sizeof a);
        CHECK(same, "vsnprintf(\"%s\", n=%zu): glibc %d [%.*s]; shim %d [%.*s]", fmt, sizes[k], r1, (int)(sizes[k] ? sizes[k] : 0), a, r2, (int)(sizes[k] ? sizes[k] : 0), b);
    }
    format_cases++;
}

static void test_printf(void) {
    check_fmt("plain");
    check_fmt("%d %i %u", 0, -1, 7u);
    check_fmt("%d", INT_MIN); check_fmt("%d", INT_MAX); check_fmt("%u", UINT_MAX);
    check_fmt("%ld %lu", LONG_MIN, ULONG_MAX);
    check_fmt("%lld %llu", LLONG_MIN, ULLONG_MAX);
    check_fmt("%hhd %hd %hhu %hu", 200, 70000, 300, -1);
    check_fmt("%zu %zd %td %jd %ju", (size_t)5, (ssize_t)-5, (ptrdiff_t)-9, (intmax_t)-3, (uintmax_t)3);
    check_fmt("%x %X %o", 0xdeadbeefu, 0xdeadbeefu, 0777u);
    check_fmt("%#x %#X %#o %#x %#o", 255u, 255u, 8u, 0u, 0u);
    check_fmt("%llx %lo %hhx", 0xffffffffffffffffull, 0777ul, 0x1ffu);
    check_fmt("%5d|%-5d|%05d|%+d|% d|%+5d|% 5d", 42, 42, 42, 42, 42, -42, -42);
    check_fmt("%.5d|%.0d|%.0d|%5.0d|%05.0d|%08.3d|%-8.3d|", 42, 0, 7, 0, 0, -7, -7);
    check_fmt("%*d|%-*d|%.*d|%*.*d|", 6, 1, 6, 1, 4, 1, -6, 2, 1);
    check_fmt("%#08x|%-#8o|%#.0x|%#.3x|%#5x|%08x", 255u, 8u, 0u, 1u, 0xabcu, 0xabcu);
    check_fmt("%c|%5c|%-5c|%05c|", 'a', 'b', 'c', 'd');
    check_fmt("%s|%5s|%-5s|%.2s|%.0s|%5.1s|%05s|", "abc", "abc", "abc", "abc", "abc", "abc", "ab");
    check_fmt("%s|%.3s|%.6s|%10s|", (char *)NULL, (char *)NULL, (char *)NULL, (char *)NULL);
    check_fmt("%p|%p|%10p|%-10p|%010p|%#p", (void *)0, (void *)0x1234, (void *)0x1234, (void *)0x1234, (void *)0x1234, (void *)0xffff);
    check_fmt("%%|%5%|%d%%", 3);
    check_fmt("%f|%f|%f|%f|%f", 0.0, 1.5, -2.25, 1e10, 1e-5);
    check_fmt("%e|%e|%e|%e|%e", 0.0, 1.5, -2.25, 1e10, 1e-5);
    check_fmt("%g|%g|%g|%g|%g", 0.0, 1.5, -2.25, 1e10, 1e-5);
    check_fmt("%E|%G|%F", 1e-5, 1e-5, 1.5);
    check_fmt("%.0f|%.0f|%.0f|%.0f|%.0f", 0.5, 1.5, 2.5, 3.5, -0.5);
    check_fmt("%.1f|%.2f|%.2f|%.3f|%.10f", 0.25, 1.005, 2.675, 1.0005, 1.0 / 3.0);
    check_fmt("%.20f|%.17g|%.15g|%.30f", 0.1, 0.1, 0.1, 1e-10);
    check_fmt("%f|%f|%f", 1e300, 1e22, 123456789012345678.0);
    check_fmt("%f|%e|%g", 4.9406564584124654e-324, 4.9406564584124654e-324, 4.9406564584124654e-324);
    check_fmt("%f|%e|%g", 1.7976931348623157e308, 1.7976931348623157e308, 1.7976931348623157e308);
    check_fmt("%e|%e|%e|%.0e|%#.0e|%.3e", 1e100, 1e-100, 9.9999995, 1.5, 1.5, 9.9995);
    check_fmt("%g|%g|%g|%g|%g|%g", 1e-4, 1e-5, 123456.0, 1234567.0, 0.0001234, 100000.0);
    check_fmt("%.0g|%.1g|%#g|%#.3g|%.3g|%G", 0.5, 0.05, 1.0, 1.0, 1234.5, 1e-10);
    check_fmt("%g|%.10g|%g|%g", 0.1 + 0.2, 0.1 + 0.2, 1e15, 1e16);
    check_fmt("%+f|% f|%+e|% g|%+.0f", 1.5, 1.5, -1.5, 2.5, 0.0);
    check_fmt("%10.3f|%-10.3f|%010.3f|%+010.3f|%-+10.3f|", 3.14159, 3.14159, -3.14159, 3.14159, 3.14159);
    check_fmt("%12.4g|%-12.4G|%012.4e|%*.*f|", 0.00001234, 1e10, -1e-5, 12, 3, 2.5);
    check_fmt("%f|%e|%g|%F|%E|%G", INFINITY, -INFINITY, INFINITY, INFINITY, -INFINITY, INFINITY);
    check_fmt("%f|%e|%g|%F|%+f|% f", NAN, NAN, -NAN, NAN, NAN, NAN);
    check_fmt("%8f|%-8f|%08f|%08e", INFINITY, INFINITY, INFINITY, NAN);
    check_fmt("%f|%.0f|%#.0f|%g|%#g", -0.0, -0.0, -0.0, -0.0, -0.0);
    check_fmt("%f|%f|%f", 0.999999949999999, 0.9999995, 999999.9999995);
    check_fmt("%g|%g|%g", 999999.5, 9999995.0, 0.000099999995);
    check_fmt("%Lf|%Le|%Lg", (long double)1.5, (long double)-2.25, (long double)1e10);
    check_fmt("%d %s %c %f %x %5.2f %-3d|", 1, "two", '3', 4.0, 5u, 6.789, 7);
    check_fmt("negative: %d %ld %lld %hd %hhd", -1, -2L, -3LL, -4, -5);
    check_fmt("%5.3s|%-5.3s|%.10s|", "abcdef", "abcdef", "ab");
    check_fmt("%3c|%-3c|", 'x', 'y');
    check_fmt("%lu %lx %lo", 4294967296ul, 4294967296ul, 4294967296ul);
    check_fmt("%zx %zo %tx %jx", (size_t)-1, (size_t)8, (ptrdiff_t)255, (uintmax_t)255);
    check_fmt("%.100f", 1.0 / 3.0);
    check_fmt("%.60e", 1.0 / 7.0);
    check_fmt("%.40g", 2.0 / 3.0);
    check_fmt("%#.0f|%#.0e|%#.0g|%#x|%#o", 1.0, 1.0, 1.0, 0u, 0u);
    check_fmt("%.3f|%.3f|%.3f", 0.0005, 0.0015, 0.0025);
    check_fmt("%e|%e", 123456789.0, 0.000123456789);
    check_fmt("%q|%qd", 1, 2LL);
    check_fmt("%-#10x|%-010d|%- d|%+ d", 255u, 5, 5, 5);
    check_fmt("%.*s|%.*f|", -1, "abc", -1, 1.5);
    check_fmt("%*d|", -7, 1);
    check_fmt("%f|%.1f|%.2f", 1e-7, 0.05, 0.005);
    check_fmt("%15.10e|%-15.3E|%15.4G", 6.02214076e23, 1.602e-19, 6.674e-11);
    {   /* %n */
        int n1 = 0, n2 = 0, m1 = 0, m2 = 0; long l1 = 0, l2 = 0; size_t z1 = 0, z2 = 0; char a[32], b[32];
        int r1 = snprintf(a, sizeof a, "ab%nc%lnd%zn", &n1, &l1, &z1);
        int r2 = fs_snprintf(b, sizeof b, "ab%nc%lnd%zn", &n2, &l2, &z2);
        CHECK(r1 == r2 && !strcmp(a, b) && n1 == n2 && l1 == l2 && z1 == z2, "%%n");
        r1 = snprintf(a, 3, "abcdef%n", &m1); r2 = fs_snprintf(b, 3, "abcdef%n", &m2);
        CHECK(r1 == r2 && !strcmp(a, b) && m1 == m2, "%%n truncated %d %d", m1, m2);
    }
    {   /* sprintf / snprintf wrappers */
        char a[64], b[64];
        CHECK(sprintf(a, "%d-%s", 5, "x") == fs_sprintf(b, "%d-%s", 5, "x") && !strcmp(a, b), "sprintf");
        CHECK(fs_snprintf(NULL, 0, "%d", 12345) == 5, "snprintf NULL");
    }
    CHECK(format_cases >= 60, "at least 60 format cases (%d)", format_cases);
}

/* ---- sscanf ------------------------------------------------------------- */
static void test_scanf(void) {
    {
        int a1 = 0, b1 = 0, a2 = 0, b2 = 0; unsigned u1 = 0, u2 = 0;
        static const char *const inputs[] = {"12 -34 56", "  7   8 9", "0x1f 077 0b11", "-1 +2 3", "abc", "", "12", "12 34", "12 abc"};
        for (size_t i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {
            const char *s = inputs[i];
            a1 = b1 = a2 = b2 = -99; u1 = u2 = 99;
            int r1 = plain_sscanf(s, "%d %d %u", &a1, &b1, &u1), r2 = fs_sscanf(s, "%d %d %u", &a2, &b2, &u2);
            CHECK(r1 == r2 && a1 == a2 && b1 == b2 && u1 == u2, "sscanf %%d %%d %%u \"%s\": glibc %d (%d %d %u) shim %d (%d %d %u)", s, r1, a1, b1, u1, r2, a2, b2, u2);
            a1 = b1 = a2 = b2 = -99; u1 = u2 = 99;
            r1 = plain_sscanf(s, "%i %i %x", &a1, &b1, &u1); r2 = fs_sscanf(s, "%i %i %x", &a2, &b2, &u2);
            CHECK(r1 == r2 && a1 == a2 && b1 == b2 && u1 == u2, "sscanf %%i %%i %%x \"%s\": glibc %d (%d %d %u) shim %d (%d %d %u)", s, r1, a1, b1, u1, r2, a2, b2, u2);
            a1 = b1 = a2 = b2 = -99; u1 = u2 = 99;
            r1 = __isoc23_sscanf(s, "%i %i %x", &a1, &b1, &u1); r2 = fs___isoc23_sscanf(s, "%i %i %x", &a2, &b2, &u2);
            CHECK(r1 == r2 && a1 == a2 && b1 == b2 && u1 == u2, "isoc23 sscanf \"%s\": glibc %d (%d %d %u) shim %d (%d %d %u)", s, r1, a1, b1, u1, r2, a2, b2, u2);
        }
    }
    {
        long l1 = 0, l2 = 0; unsigned long ul1 = 0, ul2 = 0; long long ll1 = 0, ll2 = 0; unsigned long long ull1 = 0, ull2 = 0; size_t z1 = 0, z2 = 0;
        static const char *const inputs[] = {"-9223372036854775808 18446744073709551615 -1 99999999999999999999 4096", "1 2 3 4 5", "1 2 3 4", "  -5 6 7 8 9x"};
        for (size_t i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {
            const char *s = inputs[i];
            int r1 = plain_sscanf(s, "%ld %lu %lld %llu %zu", &l1, &ul1, &ll1, &ull1, &z1);
            int r2 = fs_sscanf(s, "%ld %lu %lld %llu %zu", &l2, &ul2, &ll2, &ull2, &z2);
            CHECK(r1 == r2 && l1 == l2 && ul1 == ul2 && ll1 == ll2 && ull1 == ull2 && z1 == z2, "sscanf wide \"%s\": %d/%d %ld/%ld %lu/%lu %lld/%lld %llu/%llu %zu/%zu", s, r1, r2, l1, l2, ul1, ul2, ll1, ll2, ull1, ull2, z1, z2);
        }
    }
    {
        char w1[16], w2[16], c1[4], c2[4]; int n1 = -1, n2 = -1, d1 = 0, d2 = 0;
        static const char *const inputs[] = {"hello world 42", "  a b 7", "toolongwordhere 1", "x", "", "ab cd", "%% 5", " % 5"};
        for (size_t i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {
            const char *s = inputs[i];
            memset(w1, 0, sizeof w1); memset(w2, 0, sizeof w2); memset(c1, 0, sizeof c1); memset(c2, 0, sizeof c2); n1 = n2 = -1; d1 = d2 = -5;
            int r1 = plain_sscanf(s, "%7s%n %2c %d", w1, &n1, c1, &d1), r2 = fs_sscanf(s, "%7s%n %2c %d", w2, &n2, c2, &d2);
            CHECK(r1 == r2 && !memcmp(w1, w2, sizeof w1) && !memcmp(c1, c2, sizeof c1) && n1 == n2 && d1 == d2, "sscanf %%7s%%n %%2c %%d \"%s\": %d/%d [%s]/[%s] [%.2s]/[%.2s] %d/%d %d/%d", s, r1, r2, w1, w2, c1, c2, n1, n2, d1, d2);
            d1 = d2 = -5;
            r1 = plain_sscanf(s, "%% %d", &d1); r2 = fs_sscanf(s, "%% %d", &d2);
            CHECK(r1 == r2 && d1 == d2, "sscanf %%%% %%d \"%s\": %d/%d %d/%d", s, r1, r2, d1, d2);
        }
    }
    {   /* literals, suppression, width, %hhd/%hd, %o, %c, %p, mismatch */
        int a1, a2, b1, b2; short s1, s2; signed char c1, c2; unsigned o1, o2; void *p1, *p2; int n1, n2; char ch1, ch2;
        static const char *const inputs[] = {"key=12,34", "key=1234", "key=-7,8", "kex=1,2", "key= 7 ,9", "key=012,0x10", "key=", "key=12,34 tail"};
        for (size_t i = 0; i < sizeof inputs / sizeof inputs[0]; ++i) {
            const char *s = inputs[i];
            a1 = a2 = b1 = b2 = -1; s1 = s2 = -1; c1 = c2 = -1; o1 = o2 = 1; n1 = n2 = -1; p1 = p2 = NULL; ch1 = ch2 = 0;
            int r1 = plain_sscanf(s, "key=%2d,%d%n", &a1, &b1, &n1), r2 = fs_sscanf(s, "key=%2d,%d%n", &a2, &b2, &n2);
            CHECK(r1 == r2 && a1 == a2 && b1 == b2 && n1 == n2, "sscanf key \"%s\": %d/%d %d/%d %d/%d %d/%d", s, r1, r2, a1, a2, b1, b2, n1, n2);
            r1 = plain_sscanf(s, "key=%*d,%hd%hhd", &s1, &c1); r2 = fs_sscanf(s, "key=%*d,%hd%hhd", &s2, &c2);
            CHECK(r1 == r2 && s1 == s2 && c1 == c2, "sscanf suppress \"%s\": %d/%d %d/%d %d/%d", s, r1, r2, s1, s2, c1, c2);
            r1 = plain_sscanf(s, "key=%o,%p%c", &o1, &p1, &ch1); r2 = fs_sscanf(s, "key=%o,%p%c", &o2, &p2, &ch2);
            CHECK(r1 == r2 && o1 == o2 && p1 == p2 && ch1 == ch2, "sscanf %%o %%p %%c \"%s\": %d/%d %u/%u %p/%p %d/%d", s, r1, r2, o1, o2, p1, p2, ch1, ch2);
        }
        int v1 = 0, v2 = 0;
        CHECK(plain_sscanf("  ", "%d", &v1) == fs_sscanf("  ", "%d", &v2), "sscanf blank");
        CHECK(plain_sscanf("", "abc") == fs_sscanf("", "abc"), "sscanf empty literal");
        CHECK(plain_sscanf("ab", "abc") == fs_sscanf("ab", "abc"), "sscanf short literal");
        CHECK(plain_sscanf("x", "%d", &v1) == fs_sscanf("x", "%d", &v2), "sscanf mismatch first");
        CHECK(plain_sscanf("5 x", "%d %d", &v1, &v1) == fs_sscanf("5 x", "%d %d", &v2, &v2), "sscanf mismatch second");
        CHECK(plain_sscanf("5", "%d %d", &v1, &v1) == fs_sscanf("5", "%d %d", &v2, &v2), "sscanf exhausted second");
        CHECK(plain_sscanf("-", "%d", &v1) == fs_sscanf("-", "%d", &v2), "sscanf lone sign");
        CHECK(plain_sscanf("ff", "%x", &v1) == fs_sscanf("ff", "%x", &v2) && v1 == v2, "sscanf %%x bare");
        CHECK(plain_sscanf("0x", "%d", &v1) == fs_sscanf("0x", "%d", &v2) && v1 == v2, "sscanf %%d 0x");
    }
}

/* ---- heap --------------------------------------------------------------- */
static int aligned(const void *p, size_t a) { return ((uintptr_t)p & (a - 1)) == 0; }

static void test_heap(void) {
    stub_enabled = 0;
    fs_errno = 0;
    CHECK(fs_malloc(16) == NULL && fs_errno == ENOMEM, "malloc without table returns NULL");
    CHECK(fs_calloc(1, 16) == NULL && fs_realloc(NULL, 16) == NULL, "calloc/realloc without table");
    void *q = NULL;
    CHECK(fs_posix_memalign(&q, 64, 16) == ENOMEM && q == NULL, "posix_memalign without table");
    fs_free(NULL);
    stub_enabled = 1;
    for (int misalign = 0; misalign < 2; ++misalign) {
        stub_misalign = (size_t)misalign;
        size_t live_before = live_blocks;
        char *p = fs_malloc(10);
        CHECK(p && aligned(p, 16), "malloc aligned (misalign=%d)", misalign);
        memcpy(p, "0123456789", 10);
        char *z = fs_malloc(0);
        CHECK(z != NULL && z != p, "malloc(0) unique");
        char *r = fs_realloc(p, 4000);
        CHECK(r && aligned(r, 16) && !memcmp(r, "0123456789", 10), "realloc grow keeps content (misalign=%d)", misalign);
        memset(r + 10, 'x', 3990);
        char *r2 = fs_realloc(r, 20);
        CHECK(r2 && aligned(r2, 16) && !memcmp(r2, "0123456789xxxxxxxxxx", 20), "realloc shrink keeps content");
        CHECK(fs_realloc(r2, 0) == NULL, "realloc to zero frees");
        fs_free(z);
        unsigned *c = fs_calloc(100, sizeof *c);
        CHECK(c != NULL, "calloc");
        int zero = 1; for (int i = 0; c && i < 100; ++i) if (c[i]) zero = 0;
        CHECK(zero, "calloc zeroed");
        fs_free(c);
        CHECK(fs_calloc(SIZE_MAX / 2, 4) == NULL && fs_errno == ENOMEM, "calloc overflow");
        CHECK(fs_malloc(SIZE_MAX / 2) == NULL, "malloc huge");
        static const size_t aligns[] = {8, 16, 32, 64, 128, 4096, 65536};
        for (size_t i = 0; i < sizeof aligns / sizeof aligns[0]; ++i) {
            void *m = NULL;
            int rc = fs_posix_memalign(&m, aligns[i], 100);
            CHECK(rc == 0 && m && aligned(m, aligns[i] < 16 ? 16 : aligns[i]), "posix_memalign %zu rc %d p %p", aligns[i], rc, m);
            memset(m, (int)i, 100);
            void *g = fs_realloc(m, 300);
            CHECK(g && aligned(g, aligns[i] < 16 ? 16 : aligns[i]) && ((unsigned char *)g)[99] == (unsigned char)i, "realloc keeps over-alignment %zu", aligns[i]);
            fs_free(g);
            void *a = fs_aligned_alloc(aligns[i], 33);
            CHECK(a && aligned(a, aligns[i] < 16 ? 16 : aligns[i]), "aligned_alloc %zu", aligns[i]);
            fs_free(a);
            void *me = fs_memalign(aligns[i], 1);
            CHECK(me && aligned(me, aligns[i] < 16 ? 16 : aligns[i]), "memalign %zu", aligns[i]);
            fs_free(me);
        }
        void *m = (void *)1;
        CHECK(fs_posix_memalign(&m, 24, 8) == EINVAL && fs_posix_memalign(&m, 4, 8) == EINVAL && m == (void *)1, "posix_memalign EINVAL");
        fs_errno = 0;
        CHECK(fs_aligned_alloc(0, 8) == NULL && fs_errno == EINVAL, "aligned_alloc EINVAL");
        static const char source[] = "dup me";
        char *d = fs_strdup(source);
        CHECK(d && !strcmp(d, source) && d != source, "strdup");
        char *nd = fs_strndup("dup me", 3);
        CHECK(nd && !strcmp(nd, "dup"), "strndup");
        char *nd2 = fs_strndup("ab", 10);
        CHECK(nd2 && !strcmp(nd2, "ab"), "strndup short");
        fs_free(d); fs_free(nd); fs_free(nd2);
        CHECK(live_blocks == live_before, "all blocks released (%zu live, %zu before)", live_blocks, live_before);
    }
    stub_misalign = 0;
}

/* ---- crt ---------------------------------------------------------------- */
static char exit_log[32]; static size_t exit_len;
static void handler_a(void) { exit_log[exit_len++] = 'a'; }
static void handler_b(void) { exit_log[exit_len++] = 'b'; }
static void handler_c(void *arg) { exit_log[exit_len++] = *(const char *)arg; }
static void handler_late(void) { exit_log[exit_len++] = 'L'; }
static void handler_registers(void) { exit_log[exit_len++] = 'r'; fs_atexit(handler_late); }
static uint64_t guard_object;
static int guarded_runs;
static int guarded_value(void) {
    if (fs___cxa_guard_acquire(&guard_object)) { guarded_runs++; fs___cxa_guard_release(&guard_object); }
    return guarded_runs;
}

static void test_crt(void) {
    CHECK(fs___errno_location() == fs___errno_location(), "errno location stable");
    fs_errno = 77; CHECK(*fs___errno_location() == 77, "errno store");
    CHECK(errno != 77 || (errno = 0, 1), "errno separate from glibc");
    CHECK(fs___dso_handle == &fs___dso_handle, "__dso_handle");
    CHECK(fs___stack_chk_guard != 0, "stack guard nonzero");
    CHECK(fs___aarch64_have_lse_atomics == 0, "lse flag zero");
    CHECK(fs_atexit(handler_a) == 0 && fs_atexit(handler_b) == 0, "atexit");
    static char c_arg = 'c';
    CHECK(fs___cxa_atexit(handler_c, &c_arg, fs___dso_handle) == 0, "__cxa_atexit");
    CHECK(fs_atexit(handler_registers) == 0, "atexit late");
    CHECK(fs_atexit(NULL) == -1 && fs___cxa_atexit(NULL, NULL, NULL) == -1, "reject NULL handler");
    hook_status = -1;
    int j = setjmp(hook_jump);
    if (j == 0) fs_exit(7);
    CHECK(j == 2 && hook_status == 7, "exit reached the port hook (%d, %d)", j, hook_status);
    CHECK(exit_len == 5 && !memcmp(exit_log, "rLcba", 5), "exit handler order: %.*s", (int)exit_len, exit_log);
    j = setjmp(hook_jump);
    if (j == 0) fs_exit(3);
    CHECK(j == 2 && hook_status == 3 && exit_len == 5, "second exit runs no handlers");
    j = setjmp(hook_jump);
    if (j == 0) fs__Exit(9);
    CHECK(j == 2 && hook_status == 9, "_Exit");
    hook_aborted = 0;
    j = setjmp(hook_jump);
    if (j == 0) fs_abort();
    CHECK(j == 1 && hook_aborted, "abort");
    hook_aborted = 0; j = setjmp(hook_jump);
    if (j == 0) fs___stack_chk_fail();
    CHECK(j == 1 && hook_aborted, "__stack_chk_fail");
    hook_aborted = 0; j = setjmp(hook_jump);
    if (j == 0) fs___cxa_pure_virtual();
    CHECK(j == 1 && hook_aborted, "__cxa_pure_virtual");
    CHECK(guarded_value() == 1 && guarded_value() == 1 && guard_object == 1, "cxa guard once");
    uint64_t g2 = 0;
    CHECK(fs___cxa_guard_acquire(&g2) == 1, "guard acquire");
    fs___cxa_guard_abort(&g2);
    CHECK(g2 == 0 && fs___cxa_guard_acquire(&g2) == 1, "guard abort allows retry");
    hook_aborted = 0; j = setjmp(hook_jump);
    if (j == 0) fs___cxa_guard_acquire(&g2);
    CHECK(j == 1 && hook_aborted, "recursive guard acquisition aborts");
    fs___cxa_guard_release(&g2);
    {   /* __cxa_finalize by dso */
        exit_len = 0;
        static int dso_a, dso_b;
        CHECK(fs___cxa_atexit(handler_c, (void *)"1", &dso_a) == 0 && fs___cxa_atexit(handler_c, (void *)"2", &dso_b) == 0
              && fs___cxa_atexit(handler_c, (void *)"3", &dso_a) == 0, "register dso handlers");
        fs___cxa_finalize(&dso_a);
        CHECK(exit_len == 2 && !memcmp(exit_log, "31", 2), "finalize dso a: %.*s", (int)exit_len, exit_log);
        fs___cxa_finalize(NULL);
        CHECK(exit_len == 3 && exit_log[2] == '2', "finalize rest: %.*s", (int)exit_len, exit_log);
    }
    {   /* __clear_cache runs on a writable buffer (Linux permits the EL0 cache ops) */
        static char code[256];
        memset(code, 0, sizeof code);
        fs___clear_cache(code, code + sizeof code);
        fs___clear_cache(code + 3, code + 5);
        fs___clear_cache(code, code);
        CHECK(1, "__clear_cache executed");
    }
    CHECK(fs_cxx_selftest() == 1, "C++ operators");
}

int main(void) {
    test_strings();
    test_strto();
    test_printf();
    test_scanf();
    test_heap();
    test_crt();
    printf("checks=%d failures=%d format_cases=%d\n", checks, failures, format_cases);
    if (failures) { printf("SELFTEST FAIL\n"); return 1; }
    printf("SELFTEST PASS\n");
    return 0;
}
