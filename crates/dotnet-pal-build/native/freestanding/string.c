/* String functions. Byte semantics only (the C locale); no OS calls. */
#include "freestanding.h"

static int lower(int c) { return (c >= 'A' && c <= 'Z') ? c + ('a' - 'A') : c; }

size_t FS_NAME(strlen)(const char *s) {
    const char *p = s;
    while (*p) ++p;
    return (size_t)(p - s);
}

size_t FS_NAME(strnlen)(const char *s, size_t max) {
    size_t i = 0;
    while (i < max && s[i]) ++i;
    return i;
}

int FS_NAME(strcmp)(const char *a, const char *b) {
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    while (*x && *x == *y) { ++x; ++y; }
    return (int)*x - (int)*y;
}

int FS_NAME(strncmp)(const char *a, const char *b, size_t n) {
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    for (size_t i = 0; i < n; ++i) {
        if (x[i] != y[i]) return (int)x[i] - (int)y[i];
        if (!x[i]) return 0;
    }
    return 0;
}

int FS_NAME(strcasecmp)(const char *a, const char *b) {
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    while (*x && lower(*x) == lower(*y)) { ++x; ++y; }
    return lower(*x) - lower(*y);
}

int FS_NAME(strncasecmp)(const char *a, const char *b, size_t n) {
    const unsigned char *x = (const unsigned char *)a, *y = (const unsigned char *)b;
    for (size_t i = 0; i < n; ++i) {
        int d = lower(x[i]) - lower(y[i]);
        if (d) return d;
        if (!x[i]) return 0;
    }
    return 0;
}

char *FS_NAME(strcpy)(char *d, const char *s) {
    char *r = d;
    while ((*d++ = *s++) != 0) {}
    return r;
}

/* Copies at most n bytes and pads the remainder of d with NUL, like C99. */
char *FS_NAME(strncpy)(char *d, const char *s, size_t n) {
    size_t i = 0;
    for (; i < n && s[i]; ++i) d[i] = s[i];
    for (; i < n; ++i) d[i] = 0;
    return d;
}

char *FS_NAME(strcat)(char *d, const char *s) {
    FS_NAME(strcpy)(d + FS_NAME(strlen)(d), s);
    return d;
}

char *FS_NAME(strncat)(char *d, const char *s, size_t n) {
    char *p = d + FS_NAME(strlen)(d);
    size_t i = 0;
    for (; i < n && s[i]; ++i) p[i] = s[i];
    p[i] = 0;
    return d;
}

/* c == 0 finds the terminator, as required by C. */
char *FS_NAME(strchr)(const char *s, int c) {
    char ch = (char)c;
    for (;; ++s) {
        if (*s == ch) return (char *)s;
        if (!*s) return NULL;
    }
}

char *FS_NAME(strrchr)(const char *s, int c) {
    char ch = (char)c;
    const char *last = NULL;
    for (;; ++s) {
        if (*s == ch) last = s;
        if (!*s) return (char *)last;
    }
}

/* Naive search; an empty needle matches at h. */
char *FS_NAME(strstr)(const char *h, const char *n) {
    if (!*n) return (char *)h;
    for (; *h; ++h) {
        size_t i = 0;
        while (n[i] && h[i] == n[i]) ++i;
        if (!n[i]) return (char *)h;
    }
    return NULL;
}

size_t FS_NAME(strspn)(const char *s, const char *accept) {
    size_t i = 0;
    for (; s[i]; ++i) if (!FS_NAME(strchr)(accept, s[i])) break;
    return i;
}

size_t FS_NAME(strcspn)(const char *s, const char *reject) {
    size_t i = 0;
    for (; s[i]; ++i) if (FS_NAME(strchr)(reject, s[i])) break;
    return i;
}

char *FS_NAME(strpbrk)(const char *s, const char *accept) {
    s += FS_NAME(strcspn)(s, accept);
    return *s ? (char *)s : NULL;
}

char *FS_NAME(strtok_r)(char *s, const char *delim, char **save) {
    if (!s) s = *save;
    s += FS_NAME(strspn)(s, delim);
    if (!*s) { *save = s; return NULL; }
    char *token = s;
    s += FS_NAME(strcspn)(s, delim);
    if (*s) *s++ = 0;
    *save = s;
    return token;
}

char *FS_NAME(strdup)(const char *s) {
    size_t n = FS_NAME(strlen)(s) + 1;
    char *d = FS_NAME(malloc)(n);
    if (d) memcpy(d, s, n);
    return d;
}

char *FS_NAME(strndup)(const char *s, size_t n) {
    size_t len = FS_NAME(strnlen)(s, n);
    char *d = FS_NAME(malloc)(len + 1);
    if (!d) return NULL;
    memcpy(d, s, len);
    d[len] = 0;
    return d;
}

void *FS_NAME(memchr)(const void *p, int c, size_t n) {
    const unsigned char *b = p;
    unsigned char ch = (unsigned char)c;
    for (size_t i = 0; i < n; ++i) if (b[i] == ch) return (void *)(b + i);
    return NULL;
}

/* No message table: the text is "error <n>". glibc returns a locale message. */
static size_t error_text(int e, char *buf, size_t n) {
    return (size_t)FS_NAME(snprintf)(buf, n, "error %d", e);
}

char *FS_NAME(strerror)(int e) {
    static _Thread_local char text[32];
    error_text(e, text, sizeof text);
    return text;
}

/* GNU convention (the symbol glibc headers bind to under _GNU_SOURCE): returns
 * a string, which is buf when it fits. */
char *FS_NAME(strerror_r)(int e, char *buf, size_t n) {
    if (!n) return FS_NAME(strerror)(e);
    if (error_text(e, buf, n) >= n) return FS_NAME(strerror)(e);
    return buf;
}

/* XSI convention (glibc's __xpg_strerror_r): 0 or ERANGE when buf is too small. */
int FS_NAME(__xpg_strerror_r)(int e, char *buf, size_t n) {
    char text[32];
    size_t len = error_text(e, text, sizeof text);
    if (!n) return FS_ERANGE;
    if (len >= n) { memcpy(buf, text, n - 1); buf[n - 1] = 0; return FS_ERANGE; }
    memcpy(buf, text, len + 1);
    return 0;
}
