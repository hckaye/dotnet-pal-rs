/* Conformance test of the watches group on Linux: real changes in a scratch
 * directory, reported through the boundary and, for the same changes, by an
 * inotify instance the test keeps for itself. Both must tell the same story:
 * the same kinds, names and cookies in the same order, each for its own watch.
 * With -DPAL_HOST_TEST the same run goes against the C host table; faults 1 and
 * 3 to 7 check rejection at negotiation, fault 2 the sanitizing of a provider
 * that breaks the contract. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <ftw.h>
#include <grp.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdatomic.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/inotify.h>
#include <sys/mount.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_watches_fault;
#endif
enum { ACCESS = DOTNET_PAL_WATCH_ACCESS, MODIFY = DOTNET_PAL_WATCH_MODIFY, ATTRIBUTES = DOTNET_PAL_WATCH_ATTRIBUTES, MOVED_FROM = DOTNET_PAL_WATCH_MOVED_FROM,
    MOVED_TO = DOTNET_PAL_WATCH_MOVED_TO, CREATE = DOTNET_PAL_WATCH_CREATE, DELETE = DOTNET_PAL_WATCH_DELETE, OVERFLOW = DOTNET_PAL_WATCH_OVERFLOW,
    REMOVED = DOTNET_PAL_WATCH_REMOVED, DIRECTORY = DOTNET_PAL_WATCH_DIRECTORY, ONLY_DIRECTORY = DOTNET_PAL_WATCH_ONLY_DIRECTORY, NO_FOLLOW = DOTNET_PAL_WATCH_NO_FOLLOW,
    ALL = ACCESS | MODIFY | ATTRIBUTES | MOVED_FROM | MOVED_TO | CREATE | DELETE };
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define TIMEOUT DOTNET_PAL_TIMEOUT
#define FOREVER UINT64_MAX
#define MS UINT64_C(1000000)
static const dotnet_pal_watches_ops *w;
static char root[64];
static uint64_t now(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * UINT64_C(1000000000) + (uint64_t)t.tv_nsec; }
static void pause_ms(long ms) { struct timespec t = {ms / 1000, ms % 1000 * 1000000}; nanosleep(&t, NULL); }

/* Every call goes through one of these, so the statistics can be checked to the last count at the end:
 * open, close, add, remove and read that succeeded, and everything that did not. */
static _Atomic uint64_t tally[6];
static uint32_t counted(int index, uint32_t status) { atomic_fetch_add(&tally[status == 0 ? index : 5], 1); return status; }
static uint32_t w_open(void **watcher) { return counted(0, w->open(watcher)); }
static uint32_t w_close(void *watcher) { return counted(1, w->close(watcher)); }
static uint32_t w_add(void *watcher, const uint8_t *path, size_t length, uint32_t events, uint32_t *id) { return counted(2, w->add(watcher, path, length, events, id)); }
static uint32_t w_remove(void *watcher, uint32_t id) { return counted(3, w->remove(watcher, id)); }
static uint32_t w_read(void *watcher, uint64_t timeout, dotnet_pal_watch_event *event, size_t size) { return counted(4, w->read(watcher, timeout, event, size)); }
/* A path as the boundary takes it: counted, and followed by other bytes instead of a terminator. */
static uint32_t add_path(const dotnet_pal_watches_ops *ops, void *watcher, const char *path, uint32_t events, uint32_t *id) {
    static _Thread_local uint8_t bytes[PATH_MAX + 64];
    memset(bytes, 'X', sizeof bytes); memcpy(bytes, path, strlen(path));
    *id = 77;
    return ops ? ops->add(watcher, bytes, strlen(path), events, id) : w_add(watcher, bytes, strlen(path), events, id);
}
/* A path below the scratch directory. */
static const char *in(const char *relative) {
    static char paths[8][PATH_MAX]; static unsigned turn;
    char *path = paths[turn++ % 8];
    snprintf(path, PATH_MAX, "%s/%s", root, relative);
    return path;
}
static void touch(const char *path) { int fd = open(path, O_CREAT | O_EXCL | O_WRONLY | O_CLOEXEC, 0644); assert(fd >= 0 && close(fd) == 0); }
static void append(const char *path, const char *text) {
    int fd = open(path, O_WRONLY | O_APPEND | O_CLOEXEC);
    assert(fd >= 0 && write(fd, text, strlen(text)) == (ssize_t)strlen(text) && close(fd) == 0);
}
static int wipe_one(const char *path, const struct stat *s, int kind, struct FTW *f) { (void)s; (void)kind; (void)f; chmod(path, 0700); return remove(path); }
static int descriptors(void) {
    DIR *list = opendir("/proc/self/fd"); assert(list);
    int count = 0;
    for (struct dirent *e; (e = readdir(list));) if (e->d_name[0] != '.' && atoi(e->d_name) != dirfd(list)) ++count;
    closedir(list);
    return count;
}
static unsigned long limit_of(const char *name) {
    char path[128]; unsigned long value = 0;
    snprintf(path, sizeof path, "/proc/sys/fs/inotify/%s", name);
    FILE *f = fopen(path, "r"); assert(f && fscanf(f, "%lu", &value) == 1); fclose(f);
    return value;
}

/* The test's own reading of inotify, written apart from both providers. */
static uint32_t mask_of(uint32_t events) {
    uint32_t mask = IN_EXCL_UNLINK;
    static const uint32_t pairs[][2] = {{ACCESS, IN_ACCESS}, {MODIFY, IN_MODIFY}, {ATTRIBUTES, IN_ATTRIB}, {MOVED_FROM, IN_MOVED_FROM}, {MOVED_TO, IN_MOVED_TO},
        {CREATE, IN_CREATE}, {DELETE, IN_DELETE}, {ONLY_DIRECTORY, IN_ONLYDIR}, {NO_FOLLOW, IN_DONT_FOLLOW}};
    for (size_t i = 0; i < sizeof pairs / sizeof pairs[0]; ++i) if (events & pairs[i][0]) mask |= pairs[i][1];
    return mask;
}
static uint32_t events_of(uint32_t mask) {
    uint32_t events = 0;
    static const uint32_t pairs[][2] = {{ACCESS, IN_ACCESS}, {MODIFY, IN_MODIFY}, {ATTRIBUTES, IN_ATTRIB}, {MOVED_FROM, IN_MOVED_FROM}, {MOVED_TO, IN_MOVED_TO},
        {CREATE, IN_CREATE}, {DELETE, IN_DELETE}, {OVERFLOW, IN_Q_OVERFLOW}, {REMOVED, IN_IGNORED}};
    for (size_t i = 0; i < sizeof pairs / sizeof pairs[0]; ++i) if (mask & pairs[i][1]) events |= pairs[i][0];
    if (events && (mask & IN_ISDIR)) events |= DIRECTORY;
    return events;
}
static uint32_t status_of(int code) {
    switch (code) {
    case ENOENT: return DOTNET_PAL_NOT_FOUND;
    case EACCES: return DOTNET_PAL_ACCESS_DENIED;
    case ENOTDIR: return DOTNET_PAL_NOT_DIRECTORY;
    case ENAMETOOLONG: return DOTNET_PAL_NAME_TOO_LONG;
    case ENOSPC: return DOTNET_PAL_NO_SPACE;
    default: return DOTNET_PAL_OS_ERROR;
    }
}

/* A watcher of the boundary and the test's own inotify instance, given the same watches. A slot is one watched node:
 * the id the boundary gave it and the descriptor the kernel gave the test. */
typedef struct { void *watcher; int reference, count, skipped; struct { uint32_t id; int wd; } slots[16]; } pair_t;
typedef struct { int slot; uint32_t events, cookie; char name[256]; } seen_t;
static unsigned watchers, watches, events_seen;
static unsigned long removed_when_full;
static void pair_open(pair_t *p) {
    memset(p, 0, sizeof *p);
    p->watcher = NULL;
    assert(w_open(&p->watcher) == 0 && p->watcher);
    p->reference = inotify_init1(IN_NONBLOCK | IN_CLOEXEC); assert(p->reference >= 0);
    ++watchers;
}
static void pair_close(pair_t *p) { assert(p->skipped == 0 && w_close(p->watcher) == 0 && close(p->reference) == 0); }
/* Watches a path on both sides. Both succeed or both fail, for the same reason; a node either side already watches
 * is the node the other side already watches. */
static uint32_t pair_add(pair_t *p, const char *path, uint32_t events, int *slot) {
    uint32_t id;
    uint32_t status = add_path(NULL, p->watcher, path, events, &id);
    int wd = inotify_add_watch(p->reference, path, mask_of(events)), code = errno;
    if (status != 0) { assert(id == 0 && wd < 0 && status == status_of(code)); return status; }
    assert(id != 0 && wd > 0);
    for (int i = 0; i < p->count; ++i) {
        assert((p->slots[i].id == id) == (p->slots[i].wd == wd));
        if (p->slots[i].id == id) { *slot = i; return 0; }
    }
    assert(p->count < 16);
    p->slots[p->count].id = id; p->slots[p->count].wd = wd;
    *slot = p->count++; ++watches;
    return 0;
}
static uint32_t pair_remove(pair_t *p, int slot) {
    uint32_t status = w_remove(p->watcher, p->slots[slot].id);
    int result = inotify_rm_watch(p->reference, p->slots[slot].wd);
    assert(status == 0 ? result == 0 : status == INVALID && result < 0 && errno == EINVAL);
    return status;
}
static int blank(const dotnet_pal_watch_event *e) { static const dotnet_pal_watch_event none; return memcmp(e, &none, sizeof none) == 0; }
/* Takes what both sides have queued and holds one against the other. `expected` events are waited for, up to two
 * seconds each; after them nothing more may be there. */
static int collect(pair_t *p, seen_t *seen, int expected) {
    int count = 0;
    for (;;) {
        dotnet_pal_watch_event e; memset(&e, 0x55, sizeof e);
        uint32_t status = w_read(p->watcher, count < expected ? 2000 * MS : 0, &e, sizeof e);
        if (status == TIMEOUT) { assert(blank(&e)); break; }
        assert(status == 0 && count < 64 && e.name_length <= 255 && (e.events & ~(uint32_t)(ALL | OVERFLOW | REMOVED | DIRECTORY)) == 0);
        /* The name is a text of its length, and nothing of an earlier event is left behind it. */
        for (size_t i = 0; i < sizeof e.name; ++i) assert(i < e.name_length ? e.name[i] != 0 && e.name[i] != '/' : e.name[i] == 0);
        seen[count].slot = -1;
        for (int i = 0; i < p->count; ++i) if (p->slots[i].id == e.watch) seen[count].slot = i;
        assert((seen[count].slot == -1) == (e.watch == 0) && (e.watch == 0) == (e.events == OVERFLOW));
        seen[count].events = e.events; seen[count].cookie = e.cookie;
        memcpy(seen[count].name, e.name, sizeof e.name);
        ++count; ++events_seen;
    }
    _Alignas(struct inotify_event) char buffer[16384];
    int index = 0;
    for (;;) {
        ssize_t got = read(p->reference, buffer, sizeof buffer);
        if (got < 0) { assert(errno == EAGAIN); break; }
        for (char *at = buffer; at < buffer + got;) {
            const struct inotify_event *e = (const struct inotify_event*)at;
            at += sizeof *e + e->len;
            uint32_t events = events_of(e->mask);
            if (events == 0) { ++p->skipped; continue; } /* the kernel has words the contract has not: an unmount */
            assert(index < count);
            int slot = -1;
            for (int i = 0; i < p->count; ++i) if (p->slots[i].wd == e->wd) slot = i;
            assert(seen[index].slot == slot && seen[index].events == events && seen[index].cookie == e->cookie);
            assert(strcmp(seen[index].name, e->len ? e->name : "") == 0);
            ++index;
        }
    }
    assert(index == count);
    return count;
}
static int is(const seen_t *s, int slot, uint32_t events, const char *name) { return s->slot == slot && s->events == events && strcmp(s->name, name) == 0; }

static void changes_in_one_directory(void) {
    pair_t p; seen_t seen[64]; int a; char text[8];
    pair_open(&p);
    assert(mkdir(in("a"), 0755) == 0 && pair_add(&p, in("a"), ALL, &a) == 0);
    touch(in("a/f")); append(in("a/f"), "hello");
    int fd = open(in("a/f"), O_RDONLY | O_CLOEXEC); assert(fd >= 0 && read(fd, text, 5) == 5 && close(fd) == 0);
    assert(chmod(in("a/f"), 0600) == 0 && rename(in("a/f"), in("a/g")) == 0);
    assert(mkdir(in("a/d"), 0755) == 0 && rename(in("a/d"), in("a/e")) == 0 && rmdir(in("a/e")) == 0 && unlink(in("a/g")) == 0);
    assert(collect(&p, seen, 11) == 11);
    assert(is(&seen[0], a, CREATE, "f") && is(&seen[1], a, MODIFY, "f") && is(&seen[2], a, ACCESS, "f") && is(&seen[3], a, ATTRIBUTES, "f"));
    /* A rename is two events that share a cookie nothing else has. */
    assert(is(&seen[4], a, MOVED_FROM, "f") && is(&seen[5], a, MOVED_TO, "g") && seen[4].cookie != 0 && seen[4].cookie == seen[5].cookie);
    assert(is(&seen[6], a, CREATE | DIRECTORY, "d") && is(&seen[7], a, MOVED_FROM | DIRECTORY, "d") && is(&seen[8], a, MOVED_TO | DIRECTORY, "e"));
    assert(seen[7].cookie != 0 && seen[7].cookie == seen[8].cookie && seen[7].cookie != seen[4].cookie);
    assert(is(&seen[9], a, DELETE | DIRECTORY, "e") && is(&seen[10], a, DELETE, "g"));
    for (int i = 0; i < 11; ++i) assert(seen[i].cookie == 0 || (seen[i].events & (MOVED_FROM | MOVED_TO)));
    /* Only what was asked for is reported: a second watcher of the same directory that wants two kinds gets those two. */
    pair_t few; int b;
    pair_open(&few);
    assert(pair_add(&few, in("a"), CREATE | DELETE, &b) == 0);
    touch(in("a/h")); append(in("a/h"), "x"); assert(chmod(in("a/h"), 0600) == 0 && rename(in("a/h"), in("a/i")) == 0 && unlink(in("a/i")) == 0);
    assert(collect(&few, seen, 2) == 2 && is(&seen[0], b, CREATE, "h") && is(&seen[1], b, DELETE, "i"));
    assert(collect(&p, seen, 6) == 6);
    pair_close(&few); pair_close(&p);
}
static void moves_between_directories(void) {
    pair_t p; seen_t seen[64]; int a, b;
    pair_open(&p);
    assert(mkdir(in("b"), 0755) == 0 && mkdir(in("c"), 0755) == 0 && pair_add(&p, in("a"), ALL, &a) == 0 && pair_add(&p, in("b"), ALL, &b) == 0 && a != b);
    assert(p.slots[a].id != p.slots[b].id);
    touch(in("a/x"));
    assert(rename(in("a/x"), in("b/y")) == 0);
    assert(collect(&p, seen, 3) == 3 && is(&seen[0], a, CREATE, "x"));
    /* Each half goes to the watch of its directory, and the cookie joins them. */
    assert(is(&seen[1], a, MOVED_FROM, "x") && is(&seen[2], b, MOVED_TO, "y") && seen[1].cookie != 0 && seen[1].cookie == seen[2].cookie);
    /* Out of sight and back: one half each, with a cookie that has no partner. */
    uint32_t first = seen[1].cookie;
    assert(rename(in("b/y"), in("c/z")) == 0 && rename(in("c/z"), in("a/w")) == 0);
    assert(collect(&p, seen, 2) == 2 && is(&seen[0], b, MOVED_FROM, "y") && is(&seen[1], a, MOVED_TO, "w"));
    assert(seen[0].cookie != 0 && seen[1].cookie != 0 && seen[0].cookie != seen[1].cookie && seen[0].cookie != first);
    assert(unlink(in("a/w")) == 0 && collect(&p, seen, 1) == 1 && is(&seen[0], a, DELETE, "w"));
    pair_close(&p);
}
static void the_watched_node_itself(void) {
    pair_t p; seen_t seen[64]; int a, file;
    pair_open(&p);
    touch(in("c/file"));
    assert(pair_add(&p, in("a"), ALL, &a) == 0 && pair_add(&p, in("c/file"), MODIFY | ATTRIBUTES, &file) == 0);
    /* What happens to the watched node has no name. */
    assert(chmod(in("a"), 0700) == 0 && collect(&p, seen, 1) == 1 && is(&seen[0], a, ATTRIBUTES | DIRECTORY, ""));
    append(in("c/file"), "text"); assert(chmod(in("c/file"), 0600) == 0);
    assert(collect(&p, seen, 2) == 2 && is(&seen[0], file, MODIFY, "") && is(&seen[1], file, ATTRIBUTES, ""));
    /* The loss of the node ends the watch; the kernel reports the link count on the way. */
    assert(unlink(in("c/file")) == 0);
    int count = collect(&p, seen, 2);
    assert(count == 2 && is(&seen[0], file, ATTRIBUTES, "") && is(&seen[1], file, REMOVED, ""));
    assert(pair_remove(&p, file) == INVALID);
    pair_close(&p);
}
static void one_node_one_watch(void) {
    pair_t p; seen_t seen[64]; int a, again, link, slot; char lengthy[PATH_MAX];
    pair_open(&p);
    touch(in("a/k"));
    assert(pair_add(&p, in("a"), ALL, &a) == 0);
    /* The same path again keeps the id and takes the new events: the write is no longer reported, the entry is. */
    assert(pair_add(&p, in("a"), CREATE, &again) == 0 && again == a && p.count == 1);
    append(in("a/k"), "unseen"); touch(in("a/n"));
    assert(collect(&p, seen, 1) == 1 && is(&seen[0], a, CREATE, "n"));
    /* Another spelling and a symbolic link lead to the same node, so to the same watch: also one of 4095 bytes. */
    assert(symlink("a", in("link")) == 0);
    assert(pair_add(&p, in("a/../a/."), ALL, &again) == 0 && again == a && pair_add(&p, in("link"), ALL, &again) == 0 && again == a);
    size_t length = (size_t)snprintf(lengthy, sizeof lengthy, "%s/a", root);
    while (length + 2 <= 4095) { memcpy(lengthy + length, "/.", 3); length += 2; }
    if (length < 4095) { memmove(lengthy + 1, lengthy, length + 1); ++length; } /* a second leading slash */
    assert(strlen(lengthy) == 4095 && pair_add(&p, lengthy, ALL, &again) == 0 && again == a && p.count == 1);
    /* With NO_FOLLOW the link is a node of its own, and no directory. */
    assert(pair_add(&p, in("link"), ALL | NO_FOLLOW, &link) == 0 && link != a && p.count == 2);
    assert(pair_add(&p, in("link"), ALL | NO_FOLLOW | ONLY_DIRECTORY, &slot) == DOTNET_PAL_NOT_DIRECTORY);
    assert(pair_add(&p, in("link"), ALL | ONLY_DIRECTORY, &again) == 0 && again == a);
    touch(in("a/through"));
    assert(collect(&p, seen, 1) == 1 && is(&seen[0], a, CREATE, "through"));
    assert(unlink(in("link")) == 0);
    int count = collect(&p, seen, 2);
    assert(count >= 1 && is(&seen[count - 1], link, REMOVED, ""));
    pair_close(&p);
}
static void refusals(void) {
    pair_t p; int slot; char lengthy[400];
    pair_open(&p);
    assert(pair_add(&p, in("missing"), ALL, &slot) == DOTNET_PAL_NOT_FOUND && pair_add(&p, in("missing/below"), ALL, &slot) == DOTNET_PAL_NOT_FOUND);
    assert(pair_add(&p, in("a/k"), ALL | ONLY_DIRECTORY, &slot) == DOTNET_PAL_NOT_DIRECTORY && pair_add(&p, in("a/k/below"), ALL, &slot) == DOTNET_PAL_NOT_DIRECTORY);
    int length = snprintf(lengthy, sizeof lengthy, "%s/", root);
    memset(lengthy + length, 'n', 300); lengthy[length + 300] = 0;
    assert(pair_add(&p, lengthy, ALL, &slot) == DOTNET_PAL_NAME_TOO_LONG && p.count == 0);
    /* A file is watched as readily as a directory when nobody insists on one. */
    assert(pair_add(&p, in("a/k"), ALL, &slot) == 0);
    pair_close(&p);
}
static void waits(void) {
    pair_t p; int a; dotnet_pal_watch_event e;
    pair_open(&p);
    assert(pair_add(&p, in("a"), ALL, &a) == 0);
    memset(&e, 0x55, sizeof e);
    uint64_t begin = now();
    assert(w_read(p.watcher, 0, &e, sizeof e) == TIMEOUT && blank(&e) && now() - begin < 500 * MS);
    memset(&e, 0x55, sizeof e);
    begin = now();
    assert(w_read(p.watcher, 150 * MS, &e, sizeof e) == TIMEOUT && blank(&e));
    uint64_t waited = now() - begin;
    assert(waited >= 150 * MS && waited < 2000 * MS);
    begin = now();
    assert(w_read(p.watcher, 1, &e, sizeof e) == TIMEOUT && now() - begin < 500 * MS);
    /* What is queued is taken without a wait, whatever the limit, and a limit near the end of the range is one. */
    touch(in("a/t")); assert(unlink(in("a/t")) == 0);
    begin = now();
    assert(w_read(p.watcher, 0, &e, sizeof e) == 0 && e.events == CREATE && e.watch == p.slots[a].id && e.name_length == 1 && e.name[0] == 't');
    assert(w_read(p.watcher, FOREVER, &e, sizeof e) == 0 && e.events == DELETE && e.name[0] == 't');
    touch(in("a/t")); assert(unlink(in("a/t")) == 0);
    assert(w_read(p.watcher, FOREVER - 1, &e, sizeof e) == 0 && e.events == CREATE && w_read(p.watcher, 1, &e, sizeof e) == 0 && e.events == DELETE);
    assert(now() - begin < 500 * MS);
    events_seen += 4;
    pair_close(&p);
}
static void ends_of_a_watch(void) {
    pair_t p; seen_t seen[64]; int a, b, sub;
    pair_open(&p);
    assert(pair_add(&p, in("a"), ALL, &a) == 0 && pair_add(&p, in("b"), ALL, &b) == 0);
    /* remove queues REMOVED; afterwards the directory is silent, the other one is not, and the id names nothing. */
    assert(pair_remove(&p, a) == 0 && collect(&p, seen, 1) == 1 && is(&seen[0], a, REMOVED, ""));
    touch(in("a/silent")); touch(in("b/heard"));
    assert(collect(&p, seen, 1) == 1 && is(&seen[0], b, CREATE, "heard"));
    assert(pair_remove(&p, a) == INVALID && w_remove(p.watcher, 9999) == INVALID && w_remove(p.watcher, UINT32_MAX) == INVALID);
    /* Watched again, the directory is a new watch; whether the id of the old one comes back is the provider's affair. */
    int renewed;
    p.slots[a].id = UINT32_MAX; p.slots[a].wd = -2;
    assert(pair_add(&p, in("a"), ALL, &renewed) == 0 && renewed != a);
    /* A watched directory that is deleted: its parent reports the entry, its own watch ends. */
    assert(mkdir(in("a/sub"), 0755) == 0 && pair_add(&p, in("a/sub"), ALL, &sub) == 0);
    assert(collect(&p, seen, 1) == 1 && is(&seen[0], renewed, CREATE | DIRECTORY, "sub"));
    assert(rmdir(in("a/sub")) == 0);
    /* Which of the two the kernel queues first is its own affair; the comparison in collect holds the boundary to it. */
    assert(collect(&p, seen, 2) == 2);
    int ended = is(&seen[0], sub, REMOVED, "") ? 0 : 1;
    assert(is(&seen[ended], sub, REMOVED, "") && is(&seen[1 - ended], renewed, DELETE | DIRECTORY, "sub"));
    assert(pair_remove(&p, sub) == INVALID);
    assert(unlink(in("a/silent")) == 0 && unlink(in("b/heard")) == 0);
    assert(collect(&p, seen, 2) == 2);
    pair_close(&p);
}

struct reader { void *watcher; uint64_t timeout; int wanted; uint32_t status; int removed; dotnet_pal_watch_event event; uint64_t waited; _Atomic int done; };
/* Reads until `wanted` watches have ended, or once. */
static void *reader(void *arg) {
    struct reader *r = arg;
    uint64_t begin = now();
    do {
        memset(&r->event, 0x55, sizeof r->event);
        r->status = w_read(r->watcher, r->timeout, &r->event, sizeof r->event);
        if (r->status == 0 && (r->event.events & REMOVED)) ++r->removed;
    } while (r->status == 0 && r->removed < r->wanted);
    r->waited = now() - begin;
    atomic_store(&r->done, 1);
    return NULL;
}
static void a_reader_that_waits(void) {
    pair_t p; int a; pthread_t thread; uint32_t ids[3];
    pair_open(&p);
    assert(pair_add(&p, in("a"), ALL, &a) == 0);
    /* The consumer's way to end a wait without a limit: another thread removes the watch. */
    struct reader first = {.watcher = p.watcher, .timeout = FOREVER};
    assert(pthread_create(&thread, NULL, reader, &first) == 0);
    pause_ms(200);
    assert(!atomic_load(&first.done));
    uint64_t begin = now();
    assert(w_remove(p.watcher, p.slots[a].id) == 0 && pthread_join(thread, NULL) == 0 && now() - begin < 2000 * MS);
    assert(first.status == 0 && first.event.events == REMOVED && first.event.watch == p.slots[a].id && first.event.name_length == 0);
    /* A watch added beside the wait reports to it. */
    struct reader second = {.watcher = p.watcher, .timeout = FOREVER};
    assert(pthread_create(&thread, NULL, reader, &second) == 0);
    pause_ms(100);
    assert(!atomic_load(&second.done) && add_path(NULL, p.watcher, in("b"), CREATE, &ids[0]) == 0 && ids[0] != 0);
    pause_ms(100);
    assert(!atomic_load(&second.done));
    touch(in("b/woken"));
    assert(pthread_join(thread, NULL) == 0 && second.status == 0 && second.event.events == CREATE && second.event.watch == ids[0] && memcmp(second.event.name, "woken", 6) == 0);
    /* A wait with a limit ends with the event, not with the limit. */
    struct reader third = {.watcher = p.watcher, .timeout = 20000 * MS};
    assert(pthread_create(&thread, NULL, reader, &third) == 0);
    pause_ms(100);
    touch(in("b/again"));
    assert(pthread_join(thread, NULL) == 0 && third.status == 0 && third.event.events == CREATE && memcmp(third.event.name, "again", 6) == 0 && third.waited < 5000 * MS);
    /* The consumer's shutdown: a reader that goes on until every watch has ended, and another thread that ends them. */
    assert(add_path(NULL, p.watcher, in("a"), ALL, &ids[1]) == 0 && add_path(NULL, p.watcher, in("c"), ALL, &ids[2]) == 0);
    struct reader last = {.watcher = p.watcher, .timeout = FOREVER, .wanted = 3};
    assert(pthread_create(&thread, NULL, reader, &last) == 0);
    pause_ms(100);
    begin = now();
    for (int i = 0; i < 3; ++i) assert(!atomic_load(&last.done) && w_remove(p.watcher, ids[i]) == 0);
    assert(pthread_join(thread, NULL) == 0 && now() - begin < 2000 * MS && last.status == 0 && last.removed == 3);
    assert(unlink(in("b/woken")) == 0 && unlink(in("b/again")) == 0);
    watches += 3; events_seen += 6;
    /* The reference instance was not told any of this. */
    assert(w_close(p.watcher) == 0 && close(p.reference) == 0);
}
/* More events than the kernel queues for one instance. Returns that number, or 0 when reaching it is not cheap. */
static unsigned long overflow(void) {
    unsigned long limit = limit_of("max_queued_events");
    if (limit > 100000) return 0;
    pair_t p; seen_t seen[64]; int a, b; dotnet_pal_watch_event e;
    pair_open(&p);
    assert(pair_add(&p, in("a"), CREATE | DELETE, &a) == 0 && pair_add(&p, in("b"), CREATE, &b) == 0);
    /* Two events a round and never two alike in a row: the kernel folds an event into an identical one before it. */
    for (unsigned long i = 0; i < limit / 2 + 4; ++i) { touch(in("a/o")); assert(unlink(in("a/o")) == 0); }
    /* A watch that ends while the queue is full: whether its REMOVED still gets in is the kernel's decision (6.12
     * drops it like any other event), and the boundary says what the kernel says. */
    assert(pair_remove(&p, b) == 0);
    unsigned long taken = 0, ended = 0;
    for (;;) {
        assert(w_read(p.watcher, 0, &e, sizeof e) == 0);
        if (e.events == OVERFLOW) break;
        if (e.events == REMOVED) { assert(e.watch == p.slots[b].id); ++ended; continue; }
        assert(e.watch == p.slots[a].id && e.events == (taken % 2 ? DELETE : CREATE) && e.name_length == 1 && e.name[0] == 'o' && e.cookie == 0);
        ++taken;
    }
    /* The report names no watch and no entry, comes where the loss happened, and is the last thing queued. */
    assert(e.watch == 0 && e.cookie == 0 && e.name_length == 0 && e.name[0] == 0 && taken + ended == limit);
    assert(w_read(p.watcher, 0, &e, sizeof e) == TIMEOUT);
    events_seen += taken + ended + 1;
    _Alignas(struct inotify_event) char buffer[16384];
    unsigned long kept = 0, ignored = 0; int full = 0;
    for (ssize_t got; (got = read(p.reference, buffer, sizeof buffer)) > 0;) {
        for (char *at = buffer; at < buffer + got;) {
            const struct inotify_event *k = (const struct inotify_event*)at;
            at += sizeof *k + k->len;
            if (k->mask & IN_Q_OVERFLOW) { assert(!full && k->wd == -1); full = 1; } else { assert(!full); if (k->mask & IN_IGNORED) ++ignored; else ++kept; }
        }
    }
    assert(errno == EAGAIN && full && kept == taken && ignored == ended);
    removed_when_full = ended;
    /* The watch has not gone anywhere. */
    touch(in("a/after")); assert(unlink(in("a/after")) == 0);
    assert(collect(&p, seen, 2) == 2 && is(&seen[0], a, CREATE, "after") && is(&seen[1], a, DELETE, "after"));
    pair_close(&p);
    return limit;
}
static void two_watchers(void) {
    pair_t one, two; seen_t seen[64]; int a1, b1, a2;
    pair_open(&one); pair_open(&two);
    assert(one.watcher != two.watcher);
    assert(pair_add(&one, in("a"), ALL, &a1) == 0 && pair_add(&one, in("b"), CREATE, &b1) == 0 && pair_add(&two, in("a"), DELETE, &a2) == 0);
    touch(in("a/p")); assert(unlink(in("a/p")) == 0); touch(in("b/q")); assert(unlink(in("b/q")) == 0);
    assert(collect(&one, seen, 3) == 3 && is(&seen[0], a1, CREATE, "p") && is(&seen[1], a1, DELETE, "p") && is(&seen[2], b1, CREATE, "q"));
    assert(collect(&two, seen, 1) == 1 && is(&seen[0], a2, DELETE, "p"));
    /* An id belongs to its watcher: one that only the other watcher has names nothing here. */
    if (one.slots[b1].id != two.slots[a2].id) assert(w_remove(two.watcher, one.slots[b1].id) == INVALID);
    /* Ending one watcher's watch of a directory leaves the other's alone, and so does closing that watcher. */
    assert(pair_remove(&one, a1) == 0 && collect(&one, seen, 1) == 1 && is(&seen[0], a1, REMOVED, ""));
    touch(in("a/r")); assert(unlink(in("a/r")) == 0);
    assert(collect(&one, seen, 0) == 0 && collect(&two, seen, 1) == 1 && is(&seen[0], a2, DELETE, "r"));
    pair_close(&one);
    touch(in("a/s")); assert(unlink(in("a/s")) == 0);
    assert(collect(&two, seen, 1) == 1 && is(&seen[0], a2, DELETE, "s"));
    pair_close(&two);
}
static void validation(void) {
    pair_t p; seen_t seen[64]; int a; uint32_t id; dotnet_pal_watch_event e[2]; dotnet_pal_watches_stats before, after; void *handles[2];
    static uint8_t lengthy[4097];
    memset(lengthy, 'n', sizeof lengthy); lengthy[0] = '/';
    pair_open(&p);
    assert(pair_add(&p, in("a"), ALL, &a) == 0);
    const uint8_t *path = (const uint8_t*)in("a"); size_t length = strlen((const char*)path);
    assert(w->read_stats(&before, sizeof before) == 0);
    assert(w_open(NULL) == INVALID && w_open((void**)((char*)handles + 1)) == INVALID && w_close(NULL) == INVALID);
#define ADD(...) do { id = 77; assert(w_add(__VA_ARGS__) == INVALID); } while (0)
    ADD(NULL, path, length, ALL, &id); assert(id == 0);
    ADD(p.watcher, NULL, length, ALL, &id); assert(id == 0);
    ADD(p.watcher, path, 0, ALL, &id); assert(id == 0);
    ADD(p.watcher, lengthy, 4096, ALL, &id); assert(id == 0);
    ADD(p.watcher, (const uint8_t*)"/tmp\0x", 6, ALL, &id); assert(id == 0); /* a terminator inside the path */
    /* Nothing to report, only a way to look, and the kinds that are reported but never asked for. */
    const uint32_t kinds[] = {0, ONLY_DIRECTORY, NO_FOLLOW, ONLY_DIRECTORY | NO_FOLLOW, ALL | OVERFLOW, ALL | REMOVED, ALL | DIRECTORY, ALL | 4096u, ALL | 0x80000000u};
    for (size_t i = 0; i < sizeof kinds / sizeof kinds[0]; ++i) { ADD(p.watcher, path, length, kinds[i], &id); assert(id == 0); }
    ADD(p.watcher, path, length, ALL, NULL);
    ADD(p.watcher, path, length, ALL, (uint32_t*)((char*)e + 1));
#undef ADD
    assert(w_remove(NULL, 1) == INVALID && w_remove(p.watcher, 0) == INVALID);
    memset(e, 0x55, sizeof e); assert(w_read(NULL, 0, e, sizeof e[0]) == INVALID && blank(e));
    assert(w_read(p.watcher, 0, NULL, sizeof e[0]) == INVALID && w_read(p.watcher, 0, (dotnet_pal_watch_event*)((char*)e + 1), sizeof e[0]) == INVALID);
    memset(e, 0x55, sizeof e); assert(w_read(p.watcher, 0, e, sizeof e[0] - 1) == INVALID && e[0].watch == 0x55555555u); /* too small to write to */
    assert(w->read_stats(NULL, sizeof after) == INVALID && w->read_stats(&after, sizeof after - 1) == INVALID);
    assert(w->read_stats(&after, sizeof after) == 0 && after.rejected_or_failed == before.rejected_or_failed + 25);
    after.rejected_or_failed = before.rejected_or_failed;
    assert(memcmp(&before, &after, sizeof after) == 0);
    /* None of it reached the watcher: it has its one watch and nothing queued. */
    assert(collect(&p, seen, 0) == 0);
    touch(in("a/v")); assert(unlink(in("a/v")) == 0 && collect(&p, seen, 2) == 2);
    pair_close(&p);
}

struct verdict { uint32_t denied, allowed, opened, status, cleared, spare, reopened; int code; };
/* The limits of the target. A full descriptor table is this process's own affair. The user's limit of inotify
 * instances and a directory the user may not read need a user who is not root: a child takes an identity nobody else
 * has, so that the instances it uses up are nobody else's. Returns what could not be checked. */
static const char *limits(int *cost) {
    struct rlimit before, tight; int held[64], count = 0, fd, base = descriptors();
    void *watcher = &count;
    assert(w_open(&watcher) == 0 && watcher);
    *cost = descriptors() - base;
    assert(w_close(watcher) == 0 && descriptors() == base);
    assert(getrlimit(RLIMIT_NOFILE, &before) == 0);
    tight = before; tight.rlim_cur = 64;
    assert(setrlimit(RLIMIT_NOFILE, &tight) == 0);
    while ((fd = open("/dev/null", O_RDONLY | O_CLOEXEC)) >= 0) { assert(count < 64); held[count++] = fd; }
    assert(errno == EMFILE);
    watcher = &count;
    assert(w_open(&watcher) == DOTNET_PAL_TOO_MANY_HANDLES && watcher == NULL);
    assert(close(held[--count]) == 0 && w_open(&watcher) == 0 && watcher && w_close(watcher) == 0);
    while (count) close(held[--count]);
    assert(setrlimit(RLIMIT_NOFILE, &before) == 0 && descriptors() == base);
    watchers += 2;

    assert(mkdir(in("locked"), 0755) == 0 && mkdir(in("unlocked"), 0755) == 0 && chmod(in("locked"), 0) == 0 && chmod(root, 0755) == 0);
    if (geteuid() != 0) {
        pair_t p; int slot;
        pair_open(&p);
        assert(pair_add(&p, in("locked"), ALL, &slot) == DOTNET_PAL_ACCESS_DENIED && pair_add(&p, in("unlocked"), ALL, &slot) == 0);
        pair_close(&p);
        return "not-root";
    }
    unsigned long instances = limit_of("max_user_instances");
    int feasible = instances <= 20000 && before.rlim_cur >= instances + 64, report[2];
    assert(pipe(report) == 0);
    fflush(stdout);
    pid_t pid = fork();
    assert(pid >= 0);
    if (pid == 0) {
        struct verdict v = {0};
        uid_t nobody = 100000 + (uid_t)(getpid() % 100000);
        if (setgroups(0, NULL) != 0 || setgid(nobody) != 0 || setuid(nobody) != 0) _exit(2);
        void *own = NULL; uint32_t id;
        if (w->open(&own) != 0) _exit(3);
        int reference = inotify_init1(IN_CLOEXEC);
        v.denied = add_path(w, own, in("locked"), ALL, &id);
        v.code = inotify_add_watch(reference, in("locked"), IN_CREATE) < 0 ? errno : 0;
        v.allowed = add_path(w, own, in("unlocked"), ALL, &id);
        if (w->close(own) != 0 || close(reference) != 0) _exit(4);
        if (feasible) {
            void **all = calloc(instances + 8, sizeof *all), *last = &v;
            if (!all) _exit(5);
            while (v.opened < instances + 8 && (v.status = w->open(&last)) == 0) all[v.opened++] = last;
            v.cleared = last == NULL;
            int spare = open("/dev/null", O_RDONLY | O_CLOEXEC);
            v.spare = spare >= 0;
            /* One watcher less, and there is room for one again. */
            v.reopened = v.opened && w->close(all[v.opened - 1]) == 0 && w->open(&last) == 0;
        }
        _exit(write(report[1], &v, sizeof v) == (ssize_t)sizeof v ? 0 : 6);
    }
    struct verdict v; int status = 0;
    close(report[1]);
    ssize_t got = read(report[0], &v, sizeof v);
    close(report[0]);
    assert(waitpid(pid, &status, 0) == pid && WIFEXITED(status));
    if (WEXITSTATUS(status) == 2) return "no-other-user";
    assert(WEXITSTATUS(status) == 0 && got == (ssize_t)sizeof v);
    assert(v.denied == DOTNET_PAL_ACCESS_DENIED && v.code == EACCES && v.allowed == 0);
    if (!feasible) return "instances-not-cheap";
    /* Exactly the user's limit, then NO_SPACE with nothing written, while descriptors are still to be had. */
    assert(v.opened == instances && v.status == DOTNET_PAL_NO_SPACE && v.cleared && v.spare && v.reopened);
    return "none";
}
/* Needs the right to mount, which a container rarely has. */
static const char *unmounting(void) {
    assert(mkdir(in("mnt"), 0755) == 0);
    if (mount("pal-watches", in("mnt"), "tmpfs", 0, "size=1m") != 0) { assert(errno == EPERM || errno == EACCES); return "not-permitted"; }
    pair_t p; seen_t seen[64]; int m;
    pair_open(&p);
    assert(pair_add(&p, in("mnt"), ALL, &m) == 0);
    touch(in("mnt/inside"));
    assert(umount(in("mnt")) == 0);
    /* The kernel says IN_UNMOUNT and then IN_IGNORED. The contract has a word for the second only. */
    assert(collect(&p, seen, 2) == 2 && is(&seen[0], m, CREATE, "inside") && is(&seen[1], m, REMOVED, "") && p.skipped == 1);
    p.skipped = 0;
    pair_close(&p);
    return "checked";
}

int main(int argc, char **argv) {
    alarm(30);
#ifdef PAL_HOST_TEST
    pal_watches_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_watches_fault == 1 || pal_watches_fault >= 3) { assert(!api); printf("WATCHES malformed host %d rejected\n", pal_watches_fault); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_WATCHES_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_WATCHES);
    w = &api->watches;
    assert(w->open && w->close && w->add && w->remove && w->read && w->read_stats);
    dotnet_pal_watches_stats stats;
#ifdef PAL_HOST_TEST
    if (pal_watches_fault == 2) {
        void *watcher = &stats; uint32_t id; dotnet_pal_watch_event e;
        /* Success without a handle, a handle written by a failing call, a status open does not have; then one that holds. */
        assert(w->open(&watcher) == DOTNET_PAL_OS_ERROR && watcher == NULL);
        watcher = &stats; assert(w->open(&watcher) == DOTNET_PAL_NO_SPACE && watcher == NULL);
        watcher = &stats; assert(w->open(&watcher) == DOTNET_PAL_OS_ERROR && watcher == NULL);
        assert(w->open(&watcher) == 0 && watcher);
        /* The id of an overflow report, an id written by a failing call, a status add does not have, no status at all. */
        const uint32_t answers[] = {DOTNET_PAL_OS_ERROR, DOTNET_PAL_NOT_FOUND, DOTNET_PAL_OS_ERROR, DOTNET_PAL_OS_ERROR};
        for (int i = 0; i < 4; ++i) { id = 77; assert(w->add(watcher, (const uint8_t*)"/tmp", 4, ALL, &id) == answers[i] && id == 0); }
        /* No kind, an unknown bit, a requested-only bit, DIRECTORY alone, an overflow with a watch, an event without one,
         * a name without its terminator, with a '/', of 300 bytes, with a terminator inside, of 256 bytes. */
        for (int i = 0; i < 11; ++i) { memset(&e, 0x55, sizeof e); assert(w->read(watcher, 0, &e, sizeof e) == DOTNET_PAL_OS_ERROR && blank(&e)); }
        /* An event written by a call that ran out of time, then two statuses read does not have. */
        memset(&e, 0x55, sizeof e); assert(w->read(watcher, 0, &e, sizeof e) == TIMEOUT && blank(&e));
        for (int i = 0; i < 2; ++i) { memset(&e, 0x55, sizeof e); assert(w->read(watcher, 0, &e, sizeof e) == DOTNET_PAL_OS_ERROR && blank(&e)); }
        /* What is in order passes as it is: an overflow, the longest name with a cookie, the end of a watch. */
        dotnet_pal_watch_event lost = {0}; lost.events = OVERFLOW;
        assert(w->read(watcher, 0, &e, sizeof e) == 0 && memcmp(&e, &lost, sizeof e) == 0);
        assert(w->read(watcher, 0, &e, sizeof e) == 0 && e.watch == 3 && e.events == (MOVED_TO | DIRECTORY) && e.cookie == 9 && e.name_length == 255 && e.name[255] == 0);
        for (int i = 0; i < 255; ++i) assert(e.name[i] == 'n');
        dotnet_pal_watch_event ended = {0}; ended.watch = 3; ended.events = REMOVED;
        assert(w->read(watcher, 0, &e, sizeof e) == 0 && memcmp(&e, &ended, sizeof e) == 0);
        assert(w->remove(watcher, 3) == DOTNET_PAL_OS_ERROR && w->close(watcher) == DOTNET_PAL_OS_ERROR);
        assert(w->read_stats(&stats, sizeof stats) == 0 && stats.open_ok == 1 && stats.read_ok == 3 && stats.rejected_or_failed == 3 + 4 + 14 + 2);
        assert(stats.close_ok + stats.add_ok + stats.remove_ok == 0);
        puts("WATCHES host errors sanitized"); return 0;
    }
#endif
    for (int fd = 3; fd < 256; ++fd) close(fd);
    int opened = descriptors(), cost = 0;
    strcpy(root, "/tmp/pal-watches-XXXXXX");
    assert(mkdtemp(root));
    /* Each part gets its own watchdog: a read that never returns fails the run instead of hanging it. */
    alarm(30); changes_in_one_directory();
    alarm(30); moves_between_directories();
    alarm(30); the_watched_node_itself();
    alarm(30); one_node_one_watch();
    alarm(30); refusals();
    alarm(30); waits();
    alarm(30); ends_of_a_watch();
    alarm(30); a_reader_that_waits();
    alarm(30); unsigned long queue = overflow();
    alarm(30); two_watchers();
    alarm(30); validation();
    alarm(30); const char *unchecked = limits(&cost);
    alarm(30); const char *unmount = unmounting();
    alarm(30);
    assert(nftw(root, wipe_one, 16, FTW_DEPTH | FTW_PHYS) == 0 && access(root, F_OK) != 0 && descriptors() == opened);
    assert(w->read_stats(&stats, sizeof stats) == 0);
    /* Every watcher was closed, and every call is accounted for. */
    assert(stats.open_ok == atomic_load(&tally[0]) && stats.close_ok == atomic_load(&tally[1]) && stats.add_ok == atomic_load(&tally[2]));
    assert(stats.remove_ok == atomic_load(&tally[3]) && stats.read_ok == atomic_load(&tally[4]) && stats.rejected_or_failed == atomic_load(&tally[5]));
    assert(stats.open_ok == watchers && stats.close_ok == watchers && stats.read_ok == events_seen && stats.add_ok >= watches && cost == 1);
    alarm(0);
    printf("WATCHES PASS watchers=%u watches=%u events=%u queue_limit=%lu removed_on_full_queue=%lu unchecked=%s unmount=%s descriptors_per_watcher=%d refused=%llu\n", watchers, watches,
        events_seen, queue, removed_when_full, unchecked, unmount, cost, (unsigned long long)stats.rejected_or_failed);
    return 0;
}
