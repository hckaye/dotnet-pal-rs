/* Independent POSIX reference provider for the host-streams conformance suite.
 * Fault 1 withholds a callback; fault 2 breaks the progress contract. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <unistd.h>
int pal_streams_fault;
static uint32_t stream_write(uint32_t stream, const uint8_t *data, size_t size, size_t *written) {
    if (pal_streams_fault == 2) { *written = size + 1; return DOTNET_PAL_OK; }
    for (;;) {
        ssize_t n = write((int)stream, data, size);
        if (n < 0 && errno == EINTR) continue;
        if (n <= 0) return DOTNET_PAL_OS_ERROR;
        *written = (size_t)n; return DOTNET_PAL_OK;
    }
}
static uint32_t stream_read(uint32_t stream, uint8_t *data, size_t capacity, size_t *got) {
    for (;;) {
        ssize_t n = read((int)stream, data, capacity);
        if (n < 0 && errno == EINTR) continue;
        if (n < 0) return DOTNET_PAL_OS_ERROR;
        *got = (size_t)n; return DOTNET_PAL_OK;
    }
}
static uint32_t stream_terminal(uint32_t stream, uint32_t *out) {
    if (pal_streams_fault == 2) { *out = 9; return DOTNET_PAL_OS_ERROR; }
    *out = isatty((int)stream) == 1; return DOTNET_PAL_OK;
}
static const dotnet_pal_host_streams table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_streams), DOTNET_PAL_CAP_STREAMS},
    {stream_write, stream_read, stream_terminal, NULL},
};
static const dotnet_pal_host_streams malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_streams), DOTNET_PAL_CAP_STREAMS}, {stream_write, NULL, stream_terminal, NULL}};
const dotnet_pal_host_streams *dotnet_pal_host_streams_v2(void) { return pal_streams_fault == 1 ? &malformed : &table; }
