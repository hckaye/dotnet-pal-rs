/* Independent Linux reference provider for the host-volumes conformance suite. It
 * asks other sources than the Linux provider: /proc/self/mountinfo, which names the
 * device of every mount, so the format of a path is found by its device number
 * without a look at any other mount point, and statfs for the space. Fault 1
 * withholds a callback; fault 2 breaks the output contracts so the front end's
 * sanitizing is observable. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>
#include <sys/vfs.h>
int pal_volumes_fault;
/* A broken answer: the bytes as they are, the length the provider states and its status. */
static uint32_t broken(uint8_t *out, size_t capacity, size_t *needed, const char *bytes, size_t size, size_t stated, uint32_t status) {
    if (size <= capacity) memcpy(out, bytes, size);
    *needed = stated; return status;
}
/* The kernel writes a space, a tab, a newline and a backslash of a path as a backslash and three octal digits. */
static void decode(char *text) {
    char *to = text;
    for (const char *from = text; *from;) {
        if (from[0] == '\\' && from[1] >= '0' && from[1] <= '3' && from[2] >= '0' && from[2] <= '7' && from[3] >= '0' && from[3] <= '7') {
            *to++ = (char)((from[1] - '0') * 64 + (from[2] - '0') * 8 + (from[3] - '0')); from += 4;
        } else *to++ = *from++;
    }
    *to = 0;
}
/* One line of mountinfo: "id parent major:minor root point options [tags] - type source options". */
struct mount { char point[4 * PATH_MAX]; char type[64]; dev_t device; };
static int next(FILE *table, struct mount *m, char **line, size_t *size) {
    while (getline(line, size, table) > 0) {
        unsigned major_id, minor_id; int point = 0, end = 0;
        if (sscanf(*line, "%*d %*d %u:%u %*s %n%*s%n", &major_id, &minor_id, &point, &end) != 2 || !end || (size_t)(end - point) >= sizeof m->point) continue;
        const char *separator = strstr(*line + end, " - ");
        if (!separator || sscanf(separator + 3, "%63s", m->type) != 1) continue;
        memcpy(m->point, *line + point, (size_t)(end - point)); m->point[end - point] = 0;
        decode(m->point);
        m->device = makedev(major_id, minor_id);
        return 1;
    }
    return 0;
}
static uint32_t entry(size_t index, uint8_t *out, size_t capacity, size_t *needed) {
    if (pal_volumes_fault == 2) {
        static int step;
        switch (step++) {
        case 0: return broken(out, capacity, needed, "/mnt", 4, 4, DOTNET_PAL_OK);                 /* no terminator */
        case 1: return broken(out, capacity, needed, "/m\0t", 5, 5, DOTNET_PAL_OK);                /* terminator inside the text */
        case 2: return broken(out, capacity, needed, "", 1, 1, DOTNET_PAL_OK);                     /* empty text */
        case 3: return broken(out, capacity, needed, "/mnt", 5, 0, DOTNET_PAL_OK);                 /* no length */
        case 4: return broken(out, capacity, needed, "/mnt", 5, 5, 99u);                           /* no such status */
        case 5: return broken(out, capacity, needed, "/mnt", 5, 5, DOTNET_PAL_ACCESS_DENIED);      /* a status only status has */
        case 6: return broken(out, capacity, needed, "/mnt", 5, DOTNET_PAL_MAX_NAME + 2, DOTNET_PAL_BUFFER_TOO_SMALL); /* longer than any path */
        case 7: return broken(out, capacity, needed, "", 0, 5, DOTNET_PAL_BUFFER_TOO_SMALL);       /* too small for a text that fits */
        default: return broken(out, capacity, needed, "/mnt", 5, 5, DOTNET_PAL_NOT_FOUND);         /* outputs written by a failing call */
        }
    }
    FILE *table = fopen("/proc/self/mountinfo", "re");
    if (!table) return errno == ENOENT ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
    struct mount *m = malloc(sizeof *m); char *line = NULL; size_t size = 0; uint32_t status = DOTNET_PAL_NOT_FOUND;
    if (!m) { fclose(table); return DOTNET_PAL_OUT_OF_MEMORY; }
    while (next(table, m, &line, &size)) {
        if (index-- != 0) continue;
        size_t length = strlen(m->point) + 1;
        if (length > DOTNET_PAL_MAX_NAME + 1) { status = DOTNET_PAL_OS_ERROR; break; } /* a mount point longer than any path keeps its index */
        *needed = length;
        if (length <= capacity) memcpy(out, m->point, length);
        status = length <= capacity ? DOTNET_PAL_OK : DOTNET_PAL_BUFFER_TOO_SMALL;
        break;
    }
    free(line); free(m); fclose(table);
    return status;
}
static uint32_t status(const uint8_t *path, size_t path_length, dotnet_pal_volume_status *out, size_t out_size) {
    if (out_size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    if (pal_volumes_fault == 2) {
        static int step;
        memset(out, 0, sizeof *out); out->total_bytes = 100; out->free_bytes = 50; out->available_bytes = 25; memcpy(out->format, "ext4", 4);
        switch (step++) {
        case 0: out->free_bytes = 101; out->available_bytes = 0; return DOTNET_PAL_OK;   /* more free than there is */
        case 1: out->available_bytes = 51; return DOTNET_PAL_OK;                         /* more usable than free */
        case 2: memset(out->format, 'x', sizeof out->format); return DOTNET_PAL_OK;      /* a format without its terminator */
        case 3: return 99u;                                                              /* no such status */
        case 4: return DOTNET_PAL_BUFFER_TOO_SMALL;                                      /* a status only entry has */
        default: return DOTNET_PAL_NOT_FOUND;                                            /* outputs written by a failing call */
        }
    }
    char name[PATH_MAX]; struct statfs space; struct stat node;
    if (path_length >= sizeof name) return DOTNET_PAL_INVALID_ARGUMENT;
    memcpy(name, path, path_length); name[path_length] = 0;
    int rc; do rc = statfs(name, &space); while (rc != 0 && errno == EINTR);
    if (rc != 0 || stat(name, &node) != 0) {
        switch (errno) {
        case ENOENT: case ENOTDIR: return DOTNET_PAL_NOT_FOUND;
        case EACCES: return DOTNET_PAL_ACCESS_DENIED;
        case ENOMEM: return DOTNET_PAL_OUT_OF_MEMORY;
        default: return DOTNET_PAL_OS_ERROR;
        }
    }
    uint64_t unit = space.f_frsize ? (uint64_t)space.f_frsize : (uint64_t)space.f_bsize;
    memset(out, 0, sizeof *out);
    out->total_bytes = (uint64_t)space.f_blocks * unit; out->free_bytes = (uint64_t)space.f_bfree * unit; out->available_bytes = (uint64_t)space.f_bavail * unit;
    /* The last mount of the path's device names the format; without a table, or a mount of it, there is no name. A stacking file
     * system reports another device for its files than the table has for the mount (measured: Docker Desktop's fakeowner), so when
     * no line has the device, a second pass asks each mount point for the device of what is mounted there. */
    struct mount *m = malloc(sizeof *m); char *line = NULL; size_t size = 0; int found = 0;
    for (int pass = 0; pass < 2 && !found && m; ++pass) {
        FILE *table = fopen("/proc/self/mountinfo", "re"); struct stat mounted;
        while (table && next(table, m, &line, &size)) {
            if (pass == 0 ? m->device != node.st_dev : stat(m->point, &mounted) != 0 || mounted.st_dev != node.st_dev) continue;
            size_t length = strlen(m->type) < sizeof out->format ? strlen(m->type) : sizeof out->format - 1; /* cut to what the field holds */
            memset(out->format, 0, sizeof out->format); memcpy(out->format, m->type, length); found = 1;
        }
        if (table) fclose(table);
    }
    free(line); free(m);
    return DOTNET_PAL_OK;
}
static const dotnet_pal_host_volumes table = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_volumes), DOTNET_PAL_CAP_VOLUMES}, {entry, status, NULL}};
static const dotnet_pal_host_volumes malformed = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_volumes), DOTNET_PAL_CAP_VOLUMES}, {entry, NULL, NULL}};
const dotnet_pal_host_volumes *dotnet_pal_host_volumes_v2(void) { return pal_volumes_fault == 1 ? &malformed : &table; }
