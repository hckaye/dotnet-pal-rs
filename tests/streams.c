/* Conformance test of the streams group on Linux: the three standard streams,
 * checked against the descriptors the process actually has. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_streams_fault;
#endif
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_streams_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_streams_fault == 1) { assert(!api); puts("STREAMS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_STREAMS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_STREAMS);
    const dotnet_pal_streams_ops *s = &api->streams;
    size_t written = 7, got = 7; uint32_t terminal = 7;
#ifdef PAL_HOST_TEST
    if (pal_streams_fault == 2) {
        /* A host reporting more progress than asked, or none with success, is a broken provider. */
        assert(s->write(1, (const uint8_t*)"x", 1, &written) == DOTNET_PAL_OS_ERROR && written == 0);
        assert(s->is_terminal(1, &terminal) == DOTNET_PAL_OS_ERROR && terminal == 0);
        puts("STREAMS host errors sanitized"); return 0;
    }
#endif
    /* Output through the boundary lands on the process's own stdout: capture it through a pipe. */
    int captured[2];
    assert(pipe(captured) == 0);
    int saved = dup(STDOUT_FILENO);
    assert(saved >= 0 && dup2(captured[1], STDOUT_FILENO) >= 0);
    const char *line = "through stream one\n";
    uint32_t status = s->write(1, (const uint8_t*)line, strlen(line), &written);
    fflush(stdout);
    assert(dup2(saved, STDOUT_FILENO) >= 0);
    close(captured[1]);
    char buffer[64] = {0};
    ssize_t n = read(captured[0], buffer, sizeof buffer - 1);
    close(captured[0]); close(saved);
    assert(status == 0 && written == strlen(line) && n == (ssize_t)strlen(line) && strcmp(buffer, line) == 0);
    /* Input: feed stdin from a pipe and read it back through the boundary. */
    int input[2];
    assert(pipe(input) == 0);
    int saved_in = dup(STDIN_FILENO);
    assert(saved_in >= 0 && dup2(input[0], STDIN_FILENO) >= 0);
    assert(write(input[1], "abc", 3) == 3);
    close(input[1]);
    uint8_t in[8] = {0};
    assert(s->read(0, in, sizeof in, &got) == 0 && got == 3 && memcmp(in, "abc", 3) == 0);
    assert(s->read(0, in, sizeof in, &got) == 0 && got == 0); /* end of input */
    assert(dup2(saved_in, STDIN_FILENO) >= 0);
    close(input[0]); close(saved_in);
    /* Terminal detection agrees with isatty. */
    for (uint32_t stream = 0; stream < 3; ++stream) {
        assert(s->is_terminal(stream, &terminal) == 0);
        assert((terminal != 0) == (isatty((int)stream) == 1));
    }
    /* Argument validation. */
    assert(s->write(0, (const uint8_t*)"x", 1, &written) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->write(3, (const uint8_t*)"x", 1, &written) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->write(1, NULL, 1, &written) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->write(1, (const uint8_t*)"x", 0, &written) == 0 && written == 0);
    assert(s->write(1, (const uint8_t*)"x", 1, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->read(1, in, 1, &got) == DOTNET_PAL_INVALID_ARGUMENT);
    assert(s->read(0, in, 0, &got) == 0 && got == 0);
    assert(s->is_terminal(3, &terminal) == DOTNET_PAL_INVALID_ARGUMENT);
    dotnet_pal_streams_stats stats;
    assert(s->read_stats(&stats, sizeof stats) == 0 && stats.write_ok >= 2 && stats.read_ok >= 3 && stats.query_ok == 3 && stats.rejected_or_failed >= 6);
    printf("STREAMS PASS wrote=%zu read=%zu terminal=%u\n", written, got, terminal);
    return 0;
}
