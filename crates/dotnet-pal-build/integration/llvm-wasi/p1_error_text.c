/* The pinned P2-built System.Native error formatter references gai_strerror.
 * WASIp1 libc lacks the implementation. This implements only error-to-text
 * conversion using SDK constants; it does NOT provide DNS or pretend it worked.
 * Globalization is explicitly invariant for this managed test profile.
 */
#include <netdb.h>
#include <string.h>
const char *gai_strerror(int error) {
    switch (error) {
    case 0: return "Success";
    case EAI_AGAIN: return "Temporary name-resolution failure";
    case EAI_BADFLAGS: return "Invalid resolver flags";
    case EAI_FAIL: return "Permanent name-resolution failure";
    case EAI_FAMILY: return "Address family is not supported";
    case EAI_MEMORY: return "Insufficient memory for name resolution";
    case EAI_NONAME: return "Node or service could not be resolved";
    case EAI_SERVICE: return "Service is not supported for the socket type";
    case EAI_SOCKTYPE: return "Socket type is not supported";
    case EAI_SYSTEM: return "Name resolution encountered a system error";
#ifdef EAI_OVERFLOW
    case EAI_OVERFLOW: return "Name-resolution output buffer is too small";
#endif
#ifdef EAI_NODATA
    case EAI_NODATA: return "No address data is available";
#endif
    default: return "Unrecognized name-resolution error";
    }
}
int pal_p1_error_text_test(void) {
    return strcmp(gai_strerror(0), "Success") == 0 &&
           strcmp(gai_strerror(EAI_AGAIN), gai_strerror(EAI_FAIL)) != 0 &&
           strcmp(gai_strerror(123456), "Unrecognized name-resolution error") == 0;
}
