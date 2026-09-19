/* Independent POSIX reference provider for the host-terminal conformance suite.
 * Fault 1 withholds a callback; fault 2 reports a window without cells, a
 * control character that is no byte and statuses the group does not have. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <poll.h>
#include <pthread.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <termios.h>
#include <unistd.h>
int pal_terminal_fault;
static pthread_mutex_t lock = PTHREAD_MUTEX_INITIALIZER;
static struct termios initial;
static int have_initial;
static uint32_t failure(void) { return errno == ENOTTY ? DOTNET_PAL_NOT_FOUND : DOTNET_PAL_OS_ERROR; }
static uint32_t terminal_window_size(uint32_t stream, uint32_t *columns, uint32_t *rows) {
    if (pal_terminal_fault == 2) { *columns = 0; *rows = 43; return DOTNET_PAL_OK; }
    struct winsize size;
    if (ioctl((int)stream, TIOCGWINSZ, &size) != 0) return failure();
    *columns = size.ws_col; *rows = size.ws_row;
    return DOTNET_PAL_OK;
}
static uint32_t terminal_set_input_mode(uint32_t raw, uint32_t min_bytes, uint32_t timeout_ds, uint32_t interrupt_as_input) {
    if (pal_terminal_fault == 2) return DOTNET_PAL_BUFFER_TOO_SMALL;
    pthread_mutex_lock(&lock);
    /* Every mode starts from the settings found the first time. */
    if (!have_initial && tcgetattr(STDIN_FILENO, &initial) != 0) { uint32_t status = failure(); pthread_mutex_unlock(&lock); return status; }
    have_initial = 1;
    struct termios settings = initial;
    if (raw) {
        settings.c_iflag &= (tcflag_t)~(IXON | IXOFF | ICRNL | INLCR | IGNCR);
        settings.c_lflag &= (tcflag_t)~(ECHO | ICANON | IEXTEN);
        settings.c_cc[VMIN] = (cc_t)min_bytes; settings.c_cc[VTIME] = (cc_t)timeout_ds;
    }
    if (interrupt_as_input) settings.c_lflag &= (tcflag_t)~ISIG; else settings.c_lflag |= ISIG;
    /* From a background process group this stops the caller, which is job control's decision and not the table's. */
    int rc;
    do rc = tcsetattr(STDIN_FILENO, TCSANOW, &settings); while (rc != 0 && errno == EINTR);
    uint32_t status = rc == 0 ? DOTNET_PAL_OK : failure();
    pthread_mutex_unlock(&lock);
    return status;
}
static uint32_t terminal_input_ready(uint32_t *ready) {
    if (pal_terminal_fault == 2) { *ready = 9; return DOTNET_PAL_BUFFER_TOO_SMALL; }
    struct pollfd entry = {STDIN_FILENO, POLLIN, 0};
    int count;
    do count = poll(&entry, 1, 0); while (count < 0 && errno == EINTR);
    if (count < 0) return DOTNET_PAL_OS_ERROR;
    *ready = count > 0;
    return DOTNET_PAL_OK;
}
static uint32_t terminal_control_character(uint32_t which, uint32_t *value) {
    if (pal_terminal_fault == 2) { *value = 300; return DOTNET_PAL_OK; }
    static const int index[] = {0, VERASE, VEOL, VEOL2, VEOF};
    struct termios settings;
    if (tcgetattr(STDIN_FILENO, &settings) != 0) return failure();
    if (settings.c_cc[index[which]] == _POSIX_VDISABLE) return DOTNET_PAL_NOT_FOUND;
    *value = settings.c_cc[index[which]];
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_terminal table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_terminal), DOTNET_PAL_CAP_TERMINAL},
    {terminal_window_size, terminal_set_input_mode, terminal_input_ready, terminal_control_character, NULL},
};
static const dotnet_pal_host_terminal malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_terminal), DOTNET_PAL_CAP_TERMINAL},
    {terminal_window_size, NULL, terminal_input_ready, terminal_control_character, NULL}};
const dotnet_pal_host_terminal *dotnet_pal_host_terminal_v2(void) { return pal_terminal_fault == 1 ? &malformed : &table; }
