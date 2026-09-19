/* Independent Linux reference provider for the host-watches conformance suite:
 * inotify behind the host table. A watcher is one blocking inotify descriptor
 * and the events of its last read; poll(2) bounds the wait against a monotonic
 * deadline. add and remove use the descriptor only, so they run beside a reader.
 * Faults 1 and 3 to 6 withhold one required callback each and fault 7 offers a
 * table without the capability bit; fault 2 breaks the contract on purpose, one
 * broken answer per call, so the front end's sanitizing is observable. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <stdlib.h>
#include <string.h>
#include <sys/inotify.h>
#include <time.h>
#include <unistd.h>
int pal_watches_fault;
typedef struct { int fd; size_t at, end; _Alignas(struct inotify_event) char events[16384]; } watcher_t;
static uint64_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec; }

static uint32_t watch_open(void **watcher) {
    static int call;
    if (pal_watches_fault == 2) {
        static int object;
        switch (call++) {
        case 0: *watcher = NULL; return DOTNET_PAL_OK;          /* success without a handle */
        case 1: *watcher = &object; return DOTNET_PAL_NO_SPACE; /* an output written by a failing call */
        case 2: return DOTNET_PAL_NOT_FOUND;                    /* a status open does not have */
        default: *watcher = &object; return DOTNET_PAL_OK;
        }
    }
    int fd = inotify_init1(IN_CLOEXEC);
    if (fd < 0) {
        if (errno == ENOMEM) return DOTNET_PAL_OUT_OF_MEMORY;
        if (errno != EMFILE && errno != ENFILE) return DOTNET_PAL_OS_ERROR;
        /* EMFILE is also the user's limit of inotify instances: that one leaves room for another descriptor. */
        int spare = errno == EMFILE ? open("/", O_RDONLY | O_DIRECTORY | O_CLOEXEC) : -1;
        if (spare < 0) return DOTNET_PAL_TOO_MANY_HANDLES;
        close(spare);
        return DOTNET_PAL_NO_SPACE;
    }
    watcher_t *w = calloc(1, sizeof *w);
    if (!w) { close(fd); return DOTNET_PAL_OUT_OF_MEMORY; }
    w->fd = fd; *watcher = w;
    return DOTNET_PAL_OK;
}
static uint32_t watch_close(void *watcher) {
    if (pal_watches_fault == 2) return DOTNET_PAL_TIMEOUT; /* a status only read has */
    watcher_t *w = watcher;
    int failed = close(w->fd) != 0 && errno != EINTR;
    free(w);
    return failed ? DOTNET_PAL_OS_ERROR : DOTNET_PAL_OK;
}
static uint32_t watch_add(void *watcher, const uint8_t *path, size_t path_length, uint32_t events, uint32_t *watch) {
    static int call;
    if (pal_watches_fault == 2) {
        switch (call++) {
        case 0: *watch = 0; return DOTNET_PAL_OK;        /* the id of an overflow report */
        case 1: *watch = 7; return DOTNET_PAL_NOT_FOUND; /* an output written by a failing call */
        case 2: *watch = 7; return DOTNET_PAL_TIMEOUT;   /* a status add does not have */
        default: *watch = 7; return 99u;                 /* no such status */
        }
    }
    watcher_t *w = watcher;
    char text[PATH_MAX]; /* the path is a counted borrow without a terminator */
    if (path_length >= sizeof text) return DOTNET_PAL_NAME_TOO_LONG;
    memcpy(text, path, path_length); text[path_length] = 0;
    uint32_t mask = IN_EXCL_UNLINK;
    if (events & DOTNET_PAL_WATCH_ACCESS) mask |= IN_ACCESS;
    if (events & DOTNET_PAL_WATCH_MODIFY) mask |= IN_MODIFY;
    if (events & DOTNET_PAL_WATCH_ATTRIBUTES) mask |= IN_ATTRIB;
    if (events & DOTNET_PAL_WATCH_MOVED_FROM) mask |= IN_MOVED_FROM;
    if (events & DOTNET_PAL_WATCH_MOVED_TO) mask |= IN_MOVED_TO;
    if (events & DOTNET_PAL_WATCH_CREATE) mask |= IN_CREATE;
    if (events & DOTNET_PAL_WATCH_DELETE) mask |= IN_DELETE;
    if (events & DOTNET_PAL_WATCH_ONLY_DIRECTORY) mask |= IN_ONLYDIR;
    if (events & DOTNET_PAL_WATCH_NO_FOLLOW) mask |= IN_DONT_FOLLOW;
    int wd = inotify_add_watch(w->fd, text, mask);
    if (wd > 0) { *watch = (uint32_t)wd; return DOTNET_PAL_OK; }
    switch (errno) {
    case ENOENT: return DOTNET_PAL_NOT_FOUND;
    case EACCES: return DOTNET_PAL_ACCESS_DENIED;
    case ENOTDIR: return DOTNET_PAL_NOT_DIRECTORY;
    case ENAMETOOLONG: return DOTNET_PAL_NAME_TOO_LONG;
    case ENOSPC: return DOTNET_PAL_NO_SPACE;
    case ENOMEM: return DOTNET_PAL_OUT_OF_MEMORY;
    default: return DOTNET_PAL_OS_ERROR;
    }
}
static uint32_t watch_remove(void *watcher, uint32_t watch) {
    if (pal_watches_fault == 2) return DOTNET_PAL_NOT_FOUND; /* a status only add has */
    watcher_t *w = watcher;
    if (watch > INT_MAX) return DOTNET_PAL_INVALID_ARGUMENT;
    if (inotify_rm_watch(w->fd, (int)watch) == 0) return DOTNET_PAL_OK;
    return errno == EINVAL ? DOTNET_PAL_INVALID_ARGUMENT : DOTNET_PAL_OS_ERROR;
}
/* Fault 2: the broken events first, then three that are in order, so that the test sees both sides of every rule. */
static uint32_t breach(dotnet_pal_watch_event *event) {
    static int call;
    memset(event, 0, sizeof *event);
    event->watch = 3; event->events = DOTNET_PAL_WATCH_CREATE; event->name_length = 3; memcpy(event->name, "new", 3);
    switch (call++) {
    case 0: event->events = 0; return DOTNET_PAL_OK;                                                    /* no kind */
    case 1: event->events |= 4096u; return DOTNET_PAL_OK;                                               /* a bit the contract does not have */
    case 2: event->events |= DOTNET_PAL_WATCH_ONLY_DIRECTORY; return DOTNET_PAL_OK;                     /* a bit that is requested, never reported */
    case 3: event->events = DOTNET_PAL_WATCH_DIRECTORY; return DOTNET_PAL_OK;                           /* a directory nothing happened to */
    case 4: event->events = DOTNET_PAL_WATCH_OVERFLOW; return DOTNET_PAL_OK;                            /* an overflow that names a watch */
    case 5: event->watch = 0; return DOTNET_PAL_OK;                                                     /* an event of no watch */
    case 6: event->name[3] = 'X'; return DOTNET_PAL_OK;                                                 /* a name without its terminator */
    case 7: event->name[1] = '/'; return DOTNET_PAL_OK;                                                 /* a path where an entry name belongs */
    case 8: event->name_length = 300; return DOTNET_PAL_OK;                                             /* longer than the field */
    case 9: event->name_length = 5; memcpy(event->name, "ab\0de", 5); return DOTNET_PAL_OK;             /* a terminator inside the name */
    case 10: event->name_length = 256; memset(event->name, 'n', 256); return DOTNET_PAL_OK;             /* one byte more than an entry name has */
    case 11: return DOTNET_PAL_TIMEOUT;                                                                 /* an output written by a failing call */
    case 12: return DOTNET_PAL_WOULD_BLOCK;                                                             /* a status the group does not have */
    case 13: return DOTNET_PAL_NOT_FOUND;                                                               /* a status only add has */
    case 14: event->watch = 0; event->events = DOTNET_PAL_WATCH_OVERFLOW; event->name_length = 0; memset(event->name, 0, 3); return DOTNET_PAL_OK;
    case 15: event->name_length = 255; memset(event->name, 'n', 255); event->cookie = 9; event->events = DOTNET_PAL_WATCH_MOVED_TO | DOTNET_PAL_WATCH_DIRECTORY; return DOTNET_PAL_OK;
    default: event->events = DOTNET_PAL_WATCH_REMOVED; event->name_length = 0; memset(event->name, 0, 3); return DOTNET_PAL_OK;
    }
}
/* The next event of the last read that the contract has a word for; 0 when the buffer is used up. */
static int next_event(watcher_t *w, dotnet_pal_watch_event *out) {
    while (w->at < w->end) {
        const struct inotify_event *e = (const struct inotify_event*)(w->events + w->at);
        w->at += sizeof *e + e->len;
        memset(out, 0, sizeof *out);
        if (e->mask & IN_Q_OVERFLOW) { out->events = DOTNET_PAL_WATCH_OVERFLOW; return 1; }
        uint32_t events = 0;
        if (e->mask & IN_ACCESS) events |= DOTNET_PAL_WATCH_ACCESS;
        if (e->mask & IN_MODIFY) events |= DOTNET_PAL_WATCH_MODIFY;
        if (e->mask & IN_ATTRIB) events |= DOTNET_PAL_WATCH_ATTRIBUTES;
        if (e->mask & IN_MOVED_FROM) events |= DOTNET_PAL_WATCH_MOVED_FROM;
        if (e->mask & IN_MOVED_TO) events |= DOTNET_PAL_WATCH_MOVED_TO;
        if (e->mask & IN_CREATE) events |= DOTNET_PAL_WATCH_CREATE;
        if (e->mask & IN_DELETE) events |= DOTNET_PAL_WATCH_DELETE;
        if (e->mask & IN_IGNORED) events |= DOTNET_PAL_WATCH_REMOVED;
        if (events == 0) continue; /* IN_UNMOUNT: the IN_IGNORED behind it ends the watch */
        if (e->mask & IN_ISDIR) events |= DOTNET_PAL_WATCH_DIRECTORY;
        size_t length = e->len ? strnlen(e->name, e->len) : 0;
        if (length > 255) { out->events = DOTNET_PAL_WATCH_OVERFLOW; return 1; } /* an event nobody can be told about is a lost one */
        out->watch = (uint32_t)e->wd; out->events = events; out->cookie = e->cookie; out->name_length = (uint32_t)length;
        memcpy(out->name, e->name, length);
        return 1;
    }
    return 0;
}
static uint32_t watch_read(void *watcher, uint64_t timeout_ns, dotnet_pal_watch_event *event, size_t event_size) {
    (void)event_size;
    if (pal_watches_fault == 2) return breach(event);
    watcher_t *w = watcher;
    uint64_t start = now(), deadline = timeout_ns > UINT64_MAX - start ? UINT64_MAX : start + timeout_ns; /* UINT64_MAX: no limit */
    for (;;) {
        if (next_event(w, event)) return DOTNET_PAL_OK;
        int wait = -1; /* poll counts milliseconds: round up, and look again when it returns early */
        if (deadline != UINT64_MAX) {
            uint64_t current = now(), left = deadline > current ? deadline - current : 0;
            wait = left / 1000000 >= INT_MAX ? INT_MAX : (int)((left + 999999) / 1000000);
        }
        struct pollfd slot = {w->fd, POLLIN, 0};
        int ready = poll(&slot, 1, wait);
        if (ready < 0 && errno != EINTR) return DOTNET_PAL_OS_ERROR;
        if (ready <= 0) {
            if (deadline != UINT64_MAX && now() >= deadline) return DOTNET_PAL_TIMEOUT;
            continue;
        }
        /* Readable and one reader: this read does not block. */
        ssize_t count = read(w->fd, w->events, sizeof w->events);
        if (count < 0 && (errno == EINTR || errno == EAGAIN)) continue;
        if (count <= 0) return DOTNET_PAL_OS_ERROR;
        w->at = 0; w->end = (size_t)count;
    }
}
static const dotnet_pal_host_watches table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES},
    {watch_open, watch_close, watch_add, watch_remove, watch_read, NULL},
};
static const dotnet_pal_host_watches malformed[] = {
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES}, {watch_open, watch_close, watch_add, watch_remove, NULL, NULL}},
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES}, {NULL, watch_close, watch_add, watch_remove, watch_read, NULL}},
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES}, {watch_open, NULL, watch_add, watch_remove, watch_read, NULL}},
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES}, {watch_open, watch_close, NULL, watch_remove, watch_read, NULL}},
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), DOTNET_PAL_CAP_WATCHES}, {watch_open, watch_close, watch_add, NULL, watch_read, NULL}},
    {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_watches), 0}, {watch_open, watch_close, watch_add, watch_remove, watch_read, NULL}},
};
const dotnet_pal_host_watches *dotnet_pal_host_watches_v2(void) {
    if (pal_watches_fault == 1) return &malformed[0];
    if (pal_watches_fault >= 3 && pal_watches_fault <= 7) return &malformed[pal_watches_fault - 2];
    return &table;
}
