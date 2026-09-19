/* Freestanding C runtime shim for OS-less AArch64 images that link the .NET
 * NativeAOT runtime archive. Every definition in this directory goes through
 * FS_NAME(), so -DFREESTANDING_PREFIX=fs_ renames the whole surface (the host
 * self-test links these next to glibc that way). Nothing here calls an OS:
 * the only outside contracts are dotnet_pal_get_api (native heap) and the two
 * dotnet_pal_freestanding_* hooks the port defines.
 */
#ifndef DOTNET_PAL_FREESTANDING_H
#define DOTNET_PAL_FREESTANDING_H
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>

#ifndef FREESTANDING_PREFIX
#define FREESTANDING_PREFIX
#endif
#define FS_PASTE_(a, b) a##b
#define FS_PASTE(a, b) FS_PASTE_(a, b)
#define FS_NAME(name) FS_PASTE(FREESTANDING_PREFIX, name)
#define FS_STR_(x) #x
#define FS_STR(x) FS_STR_(x)

#if defined(__cplusplus)
#define FS_NORETURN [[noreturn]]
extern "C" {
#else
#define FS_NORETURN _Noreturn
#endif

/* Linux errno values used by the shim. No <errno.h> exists for the none target. */
#define FS_ENOMEM 12
#define FS_EINVAL 22
#define FS_ERANGE 34
#define fs_errno (*FS_NAME(__errno_location)())

/* Port hooks. Both end the image; neither returns. */
FS_NORETURN void dotnet_pal_freestanding_abort(void);
FS_NORETURN void dotnet_pal_freestanding_exit(int status);

/* string.c */
size_t FS_NAME(strlen)(const char *s);
size_t FS_NAME(strnlen)(const char *s, size_t max);
int FS_NAME(strcmp)(const char *a, const char *b);
int FS_NAME(strncmp)(const char *a, const char *b, size_t n);
int FS_NAME(strcasecmp)(const char *a, const char *b);
int FS_NAME(strncasecmp)(const char *a, const char *b, size_t n);
char *FS_NAME(strcpy)(char *d, const char *s);
char *FS_NAME(strncpy)(char *d, const char *s, size_t n);
char *FS_NAME(strcat)(char *d, const char *s);
char *FS_NAME(strncat)(char *d, const char *s, size_t n);
char *FS_NAME(strchr)(const char *s, int c);
char *FS_NAME(strrchr)(const char *s, int c);
char *FS_NAME(strstr)(const char *h, const char *n);
size_t FS_NAME(strspn)(const char *s, const char *accept);
size_t FS_NAME(strcspn)(const char *s, const char *reject);
char *FS_NAME(strpbrk)(const char *s, const char *accept);
char *FS_NAME(strtok_r)(char *s, const char *delim, char **save);
char *FS_NAME(strdup)(const char *s);
char *FS_NAME(strndup)(const char *s, size_t n);
void *FS_NAME(memchr)(const void *p, int c, size_t n);
char *FS_NAME(strerror)(int e);
char *FS_NAME(strerror_r)(int e, char *buf, size_t n);      /* GNU convention */
int FS_NAME(__xpg_strerror_r)(int e, char *buf, size_t n);  /* XSI convention */

/* strtol.c */
long FS_NAME(strtol)(const char *s, char **end, int base);
unsigned long FS_NAME(strtoul)(const char *s, char **end, int base);
long long FS_NAME(strtoll)(const char *s, char **end, int base);
unsigned long long FS_NAME(strtoull)(const char *s, char **end, int base);
long FS_NAME(__isoc23_strtol)(const char *s, char **end, int base);
unsigned long FS_NAME(__isoc23_strtoul)(const char *s, char **end, int base);
long long FS_NAME(__isoc23_strtoll)(const char *s, char **end, int base);
unsigned long long FS_NAME(__isoc23_strtoull)(const char *s, char **end, int base);
int FS_NAME(atoi)(const char *s);
long FS_NAME(atol)(const char *s);
long long FS_NAME(atoll)(const char *s);

/* printf.c */
int FS_NAME(vsnprintf)(char *buf, size_t n, const char *fmt, va_list ap);
int FS_NAME(snprintf)(char *buf, size_t n, const char *fmt, ...);
int FS_NAME(vsprintf)(char *buf, const char *fmt, va_list ap);
int FS_NAME(sprintf)(char *buf, const char *fmt, ...);

/* scanf.c */
int FS_NAME(vsscanf)(const char *s, const char *fmt, va_list ap);
int FS_NAME(sscanf)(const char *s, const char *fmt, ...);
int FS_NAME(__isoc99_vsscanf)(const char *s, const char *fmt, va_list ap);
int FS_NAME(__isoc99_sscanf)(const char *s, const char *fmt, ...);
int FS_NAME(__isoc23_vsscanf)(const char *s, const char *fmt, va_list ap);
int FS_NAME(__isoc23_sscanf)(const char *s, const char *fmt, ...);

/* crt.c */
int *FS_NAME(__errno_location)(void);
FS_NORETURN void FS_NAME(abort)(void);
FS_NORETURN void FS_NAME(exit)(int status);
FS_NORETURN void FS_NAME(_Exit)(int status);
int FS_NAME(atexit)(void (*fn)(void));
int FS_NAME(__cxa_atexit)(void (*fn)(void *), void *arg, void *dso);
void FS_NAME(__cxa_finalize)(void *dso);
FS_NORETURN void FS_NAME(__cxa_pure_virtual)(void);
FS_NORETURN void FS_NAME(__cxa_deleted_virtual)(void);
int FS_NAME(__cxa_guard_acquire)(uint64_t *guard);
void FS_NAME(__cxa_guard_release)(uint64_t *guard);
void FS_NAME(__cxa_guard_abort)(uint64_t *guard);
FS_NORETURN void FS_NAME(__stack_chk_fail)(void);
void FS_NAME(__clear_cache)(void *begin, void *end);
extern void *FS_NAME(__dso_handle);
extern uintptr_t FS_NAME(__stack_chk_guard);
extern unsigned char FS_NAME(__aarch64_have_lse_atomics);

/* heap.c */
void *FS_NAME(malloc)(size_t n);
void *FS_NAME(calloc)(size_t count, size_t size);
void *FS_NAME(realloc)(void *p, size_t n);
void FS_NAME(free)(void *p);
int FS_NAME(posix_memalign)(void **out, size_t alignment, size_t n);
void *FS_NAME(aligned_alloc)(size_t alignment, size_t n);
void *FS_NAME(memalign)(size_t alignment, size_t n);

/* Provided elsewhere (Rust compiler_builtins on the bare-metal port, glibc in
 * the host self-test). Declared so the shim can call them. */
void *memcpy(void *d, const void *s, size_t n);
void *memset(void *d, int c, size_t n);

#if defined(__cplusplus)
}
#endif
#endif
