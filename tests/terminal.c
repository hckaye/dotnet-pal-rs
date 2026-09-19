/* Conformance test of the terminal group on Linux. Neither CI nor a container
 * has a terminal, so the test makes one: a pseudo-terminal whose slave side
 * replaces descriptors 0, 1 and 2 before the table is negotiated, and whose
 * master side plays the person at the keyboard. Every answer is checked against
 * what the kernel reports for the slave. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_terminal_fault;
#endif
/* Descriptors 1 and 2 are the terminal under test: the result and a failure go to the streams the test was started with. */
static int out_fd = STDOUT_FILENO, err_fd = STDERR_FILENO;
#define CHECK(condition) do { if (!(condition)) { dprintf(err_fd, "%s:%d: failed: %s\n", __FILE__, __LINE__, #condition); _exit(1); } } while (0)
/* What the caller saw, to be compared with the counters at the end. */
static uint64_t seen[5];
static uint32_t tally(int group, uint32_t status) { seen[status == 0 ? group : 4]++; return status; }
#define SIZE(...) tally(0, t->window_size(__VA_ARGS__))
#define MODE(...) tally(1, t->set_input_mode(__VA_ARGS__))
#define READY(...) tally(2, t->input_ready(__VA_ARGS__))
#define CONTROL(...) tally(3, t->control_character(__VA_ARGS__))

static int open_terminal(int *master) {
    *master = posix_openpt(O_RDWR | O_NOCTTY);
    CHECK(*master >= 0 && grantpt(*master) == 0 && unlockpt(*master) == 0);
    const char *name = ptsname(*master);
    CHECK(name);
    int slave = open(name, O_RDWR | O_NOCTTY);
    CHECK(slave >= 0);
    return slave;
}
static void resize(int master, unsigned short columns, unsigned short rows) {
    struct winsize size = {.ws_row = rows, .ws_col = columns};
    CHECK(ioctl(master, TIOCSWINSZ, &size) == 0);
}
static void settings_of(int fd, struct termios *out) { memset(out, 0, sizeof *out); CHECK(tcgetattr(fd, out) == 0); }
/* The settings a mode must produce: always from the original ones, whatever the terminal went through in between. */
static void expect_mode(int slave, const struct termios *original, int raw, cc_t min_bytes, cc_t timeout_ds, int interrupt_as_input) {
    struct termios expected, now;
    memcpy(&expected, original, sizeof expected);
    if (raw) {
        expected.c_iflag &= (tcflag_t)~(IXON | IXOFF | ICRNL | INLCR | IGNCR);
        expected.c_lflag &= (tcflag_t)~(ECHO | ICANON | IEXTEN);
        expected.c_cc[VMIN] = min_bytes; expected.c_cc[VTIME] = timeout_ds;
    }
    if (interrupt_as_input) expected.c_lflag &= (tcflag_t)~ISIG; else expected.c_lflag |= ISIG;
    settings_of(slave, &now);
    CHECK(memcmp(&now, &expected, sizeof now) == 0);
}
static int waits(int fd, int milliseconds) {
    struct pollfd entry = {fd, POLLIN, 0};
    int count;
    do count = poll(&entry, 1, milliseconds); while (count < 0 && errno == EINTR);
    return count;
}
static void type(int master, const char *text) { CHECK(write(master, text, strlen(text)) == (ssize_t)strlen(text)); }
/* An echo reaches the master byte by byte: gather until the expected text is there. */
static void expect_echo(int master, const char *text) {
    char bytes[16];
    size_t filled = 0, wanted = strlen(text);
    while (filled < wanted) {
        CHECK(waits(master, 2000) == 1);
        ssize_t count = read(master, bytes + filled, wanted - filled);
        CHECK(count > 0);
        filled += (size_t)count;
    }
    CHECK(memcmp(bytes, text, wanted) == 0);
}
static int64_t now_ms(void) { struct timespec ts; clock_gettime(CLOCK_MONOTONIC, &ts); return (int64_t)ts.tv_sec * 1000 + ts.tv_nsec / 1000000; }

/* With input that is no terminal there is no mode to set and no editing character, but readiness still answers. */
static void input_is_a_pipe(const dotnet_pal_terminal_ops *t, int slave) {
    int feed[2];
    uint32_t value = 7, ready = 7, columns = 7, rows = 7;
    CHECK(pipe(feed) == 0 && dup2(feed[0], STDIN_FILENO) == STDIN_FILENO);
    CHECK(MODE(1, 1, 0, 0) == DOTNET_PAL_NOT_FOUND && MODE(0, 1, 0, 0) == DOTNET_PAL_NOT_FOUND);
    CHECK(CONTROL(DOTNET_PAL_CONTROL_ERASE, &value) == DOTNET_PAL_NOT_FOUND && value == 0);
    CHECK(SIZE(0, &columns, &rows) == DOTNET_PAL_NOT_FOUND && columns == 0 && rows == 0);
    CHECK(READY(&ready) == 0 && ready == 0);
    CHECK(write(feed[1], "p", 1) == 1);
    CHECK(READY(&ready) == 0 && ready == 1);
    CHECK(dup2(slave, STDIN_FILENO) == STDIN_FILENO);
    close(feed[0]); close(feed[1]);
}
/* A process group in the background of its controlling terminal is stopped by
 * SIGTTOU when it changes the terminal, as the first worker shows with a plain
 * tcsetattr; the same change through the boundary is stopped the same way and
 * leaves the terminal of the foreground job alone. The session gets a terminal
 * of its own: the one under test stays out of it. */
static void background_group(const dotnet_pal_terminal_ops *t) {
    pid_t leader = fork();
    CHECK(leader >= 0);
    if (leader == 0) {
        int own_master;
        struct termios before, after;
        alarm(30);
        CHECK(setsid() > 0);
        int own = open_terminal(&own_master);
        CHECK(ioctl(own, TIOCSCTTY, 0) == 0 && dup2(own, STDIN_FILENO) == STDIN_FILENO);
        settings_of(STDIN_FILENO, &before);
        for (int through_boundary = 0; through_boundary < 2; ++through_boundary) {
            int state;
            pid_t worker = fork();
            CHECK(worker >= 0);
            if (worker == 0) {
                alarm(30);
                if (setpgid(0, 0) != 0) _exit(2);
                if (through_boundary) _exit(t->set_input_mode(1, 1, 0, 0) == 0 ? 0 : 1);
                _exit(tcsetattr(STDIN_FILENO, TCSANOW, &before) == 0 ? 0 : 1);
            }
            CHECK(waitpid(worker, &state, WUNTRACED) == worker);
            CHECK(WIFSTOPPED(state) && WSTOPSIG(state) == SIGTTOU);
            CHECK(kill(worker, SIGKILL) == 0 && waitpid(worker, &state, 0) == worker);
        }
        settings_of(STDIN_FILENO, &after);
        CHECK((before.c_lflag & (ICANON | ECHO)) == (ICANON | ECHO) && (after.c_lflag & (ICANON | ECHO)) == (ICANON | ECHO));
        _exit(0);
    }
    int state;
    CHECK(waitpid(leader, &state, 0) == leader && WIFEXITED(state) && WEXITSTATUS(state) == 0);
}

int main(int argc, char **argv) {
    alarm(60);
#ifdef PAL_HOST_TEST
    pal_terminal_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    out_fd = fcntl(STDOUT_FILENO, F_DUPFD, 3);
    err_fd = fcntl(STDERR_FILENO, F_DUPFD, 3);
    if (out_fd < 0 || err_fd < 0) return 1;
    int master;
    int slave = open_terminal(&master);
    resize(master, 132, 43);
    for (int fd = 0; fd < 3; ++fd) CHECK(dup2(slave, fd) == fd);
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_terminal_fault == 1) { CHECK(!api); dprintf(out_fd, "TERMINAL malformed host rejected\n"); return 0; }
#endif
    CHECK(api && api->header.struct_size >= DOTNET_PAL_TERMINAL_API_SIZE);
    CHECK(api->header.capabilities & DOTNET_PAL_CAP_TERMINAL);
    const dotnet_pal_terminal_ops *t = &api->terminal;
    uint32_t columns = 7, rows = 7, ready = 7, value = 7;
#ifdef PAL_HOST_TEST
    if (pal_terminal_fault == 2) {
        /* A character that is no byte and statuses of other groups are a broken provider; a window without cells is a
         * terminal without a size, whoever reports it. */
        CHECK(t->window_size(0, &columns, &rows) == DOTNET_PAL_UNSUPPORTED && columns == 0 && rows == 0);
        CHECK(t->control_character(DOTNET_PAL_CONTROL_ERASE, &value) == DOTNET_PAL_OS_ERROR && value == 0);
        CHECK(t->set_input_mode(1, 1, 0, 0) == DOTNET_PAL_OS_ERROR);
        CHECK(t->input_ready(&ready) == DOTNET_PAL_OS_ERROR && ready == 0);
        dprintf(out_fd, "TERMINAL host errors sanitized\n"); return 0;
    }
#endif
    /* The window of each stream is the slave's, and follows it. */
    for (uint32_t stream = 0; stream < 3; ++stream) {
        columns = rows = 7;
        CHECK(SIZE(stream, &columns, &rows) == 0 && columns == 132 && rows == 43);
    }
    resize(master, 80, 24);
    CHECK(SIZE(1, &columns, &rows) == 0 && columns == 80 && rows == 24);
    CHECK(SIZE(3, &columns, &rows) == DOTNET_PAL_INVALID_ARGUMENT && columns == 0 && rows == 0);
    int redirected[2];
    CHECK(pipe(redirected) == 0 && dup2(redirected[1], STDOUT_FILENO) == STDOUT_FILENO);
    CHECK(SIZE(1, &columns, &rows) == DOTNET_PAL_NOT_FOUND && columns == 0 && rows == 0);
    CHECK(SIZE(0, &columns, &rows) == 0 && columns == 80 && rows == 24);
    CHECK(dup2(slave, STDOUT_FILENO) == STDOUT_FILENO);
    close(redirected[0]); close(redirected[1]);
    /* A terminal nobody gave a size reports none; the boundary has no cells to hand out. */
    resize(master, 0, 0);
    CHECK(SIZE(0, &columns, &rows) == DOTNET_PAL_UNSUPPORTED && columns == 0 && rows == 0);
    resize(master, 132, 43);

    /* The pseudo-terminal starts in line mode with echo and the interrupt key, which the checks below rely on. */
    struct termios original, now;
    settings_of(slave, &original);
    CHECK((original.c_lflag & (ICANON | ECHO | ISIG | IEXTEN)) == (ICANON | ECHO | ISIG | IEXTEN) && (original.c_iflag & ICRNL));
    input_is_a_pipe(t, slave);
    expect_mode(slave, &original, 0, 0, 0, 0);

    /* Raw mode: a byte arrives as typed, at once, without a newline and without an echo. */
    CHECK(MODE(1, 1, 0, 0) == 0);
    expect_mode(slave, &original, 1, 1, 0, 0);
    CHECK(READY(&ready) == 0 && ready == 0);
    type(master, "x");
    CHECK(waits(STDIN_FILENO, 2000) == 1);
    CHECK(READY(&ready) == 0 && ready == 1);
    char bytes[16] = {0};
    CHECK(read(STDIN_FILENO, bytes, sizeof bytes) == 1 && bytes[0] == 'x');
    CHECK(READY(&ready) == 0 && ready == 0);
    type(master, "\r");
    CHECK(read(STDIN_FILENO, bytes, sizeof bytes) == 1 && bytes[0] == '\r');
    CHECK(waits(master, 200) == 0);
    /* The interrupt key as a byte, and a read that gives up after two tenths of a second. */
    CHECK(MODE(1, 0, 2, 1) == 0);
    expect_mode(slave, &original, 1, 0, 2, 1);
    int64_t started = now_ms();
    CHECK(read(STDIN_FILENO, bytes, sizeof bytes) == 0 && now_ms() - started >= 150);
    type(master, "\x03");
    CHECK(waits(STDIN_FILENO, 2000) == 1 && read(STDIN_FILENO, bytes, sizeof bytes) == 1 && bytes[0] == 3);
    /* Line mode gives back the original settings bit for bit, apart from the interrupt key while it stays a byte. */
    CHECK(MODE(0, 9, 9, 1) == 0);
    expect_mode(slave, &original, 0, 0, 0, 1);
    CHECK(MODE(0, 1, 0, 0) == 0);
    settings_of(slave, &now);
    CHECK(memcmp(&now, &original, sizeof now) == 0);
    type(master, "ok\n");
    expect_echo(master, "ok\r\n");
    CHECK(waits(STDIN_FILENO, 2000) == 1);
    CHECK(READY(&ready) == 0 && ready == 1);
    CHECK(read(STDIN_FILENO, bytes, sizeof bytes) == 3 && memcmp(bytes, "ok\n", 3) == 0);
    /* Switching back and forth never drifts. */
    for (int round = 0; round < 64; ++round) {
        CHECK(MODE(1, (uint32_t)round % 7, (uint32_t)round % 5, (uint32_t)round & 1) == 0);
        expect_mode(slave, &original, 1, (cc_t)(round % 7), (cc_t)(round % 5), round & 1);
        if (round % 3 == 0) continue; /* raw mode on top of raw mode as well */
        CHECK(MODE(0, 1, 0, (uint32_t)round & 1) == 0);
        expect_mode(slave, &original, 0, 0, 0, round & 1);
    }
    CHECK(MODE(0, 1, 0, 0) == 0);
    settings_of(slave, &now);
    CHECK(memcmp(&now, &original, sizeof now) == 0);
    input_is_a_pipe(t, slave);

    /* The editing characters are the slave's; a disabled one is none. */
    now.c_cc[VEOL] = _POSIX_VDISABLE; now.c_cc[VEOL2] = 0x1d;
    CHECK(tcsetattr(slave, TCSANOW, &now) == 0);
    CHECK(CONTROL(DOTNET_PAL_CONTROL_ERASE, &value) == 0 && value == now.c_cc[VERASE] && value != _POSIX_VDISABLE);
    CHECK(CONTROL(DOTNET_PAL_CONTROL_END_OF_FILE, &value) == 0 && value == now.c_cc[VEOF] && value != _POSIX_VDISABLE);
    CHECK(CONTROL(DOTNET_PAL_CONTROL_END_OF_LINE_2, &value) == 0 && value == 0x1d);
    value = 7;
    CHECK(CONTROL(DOTNET_PAL_CONTROL_END_OF_LINE, &value) == DOTNET_PAL_NOT_FOUND && value == 0);
    uint32_t erase = now.c_cc[VERASE], end_of_file = now.c_cc[VEOF];

    /* Argument validation; a rejected call leaves the terminal alone. */
    CHECK(MODE(2, 1, 0, 0) == DOTNET_PAL_INVALID_ARGUMENT && MODE(1, 256, 0, 0) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(MODE(1, 1, 256, 0) == DOTNET_PAL_INVALID_ARGUMENT && MODE(1, 1, 0, 2) == DOTNET_PAL_INVALID_ARGUMENT);
    struct termios untouched;
    settings_of(slave, &untouched);
    CHECK(memcmp(&untouched, &now, sizeof now) == 0);
    value = 7;
    CHECK(CONTROL(0, &value) == DOTNET_PAL_INVALID_ARGUMENT && value == 0);
    CHECK(CONTROL(5, &value) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(CONTROL(DOTNET_PAL_CONTROL_ERASE, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(SIZE(0, NULL, &rows) == DOTNET_PAL_INVALID_ARGUMENT && SIZE(0, &columns, NULL) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(READY(NULL) == DOTNET_PAL_INVALID_ARGUMENT);

    background_group(t);

    /* A terminal that went away makes a read return at once, so input is ready. */
    CHECK(MODE(1, 1, 0, 0) == 0);
    CHECK(READY(&ready) == 0 && ready == 0);
    close(master);
    CHECK(waits(STDIN_FILENO, 2000) == 1);
    CHECK(READY(&ready) == 0 && ready == 1);
    CHECK(read(STDIN_FILENO, bytes, sizeof bytes) == 0);

    dotnet_pal_terminal_stats stats;
    CHECK(t->read_stats(NULL, sizeof stats) == DOTNET_PAL_INVALID_ARGUMENT && t->read_stats(&stats, sizeof stats - 1) == DOTNET_PAL_INVALID_ARGUMENT);
    CHECK(t->read_stats(&stats, sizeof stats) == 0);
    CHECK(stats.size_ok == seen[0] && stats.mode_ok == seen[1] && stats.ready_ok == seen[2] && stats.control_ok == seen[3] && stats.rejected_or_failed == seen[4]);
    dprintf(out_fd, "TERMINAL PASS window=132x43 modes=%llu ready=%llu erase=%u eof=%u rejected=%llu\n", (unsigned long long)stats.mode_ok,
        (unsigned long long)stats.ready_ok, erase, end_of_file, (unsigned long long)stats.rejected_or_failed);
    return 0;
}
