/* Independent reference provider for the host-accounts conformance suite. It asks
 * no name service: it reads /etc/passwd and /etc/group itself, and the groups of
 * this process from /proc/self/status. Fault 1 offers a table without the
 * capability bit; fault 2 breaks the output contracts so the front end's
 * sanitizing is observable; fault 3 withholds every callback. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
int pal_accounts_fault;
/* The limits of the front end, which the header does not name. */
#define LONGEST_NAME 255
#define MOST_GROUPS 65536
/* The next field of a line of either file: the text up to the ':' or the end, which it replaces by a terminator. NULL after the last one. */
static char *field(char **rest) {
    char *text = *rest;
    if (!text) return NULL;
    char *end = strchr(text, ':');
    if (end) { *end = 0; *rest = end + 1; } else *rest = NULL;
    return text;
}
static int number(const char *text, uint32_t *out) {
    char *end; errno = 0;
    unsigned long long value = strtoull(text, &end, 10);
    if (!*text || *end || errno || value > UINT32_MAX || text[0] == '-' || text[0] == '+') return 0;
    *out = (uint32_t)value; return 1;
}
static int put(uint8_t *to, size_t size, const char *text) {
    if (strlen(text) >= size) return 0;
    memcpy(to, text, strlen(text) + 1); return 1;
}
/* The first entry of /etc/passwd with the name, or without a name the first one with the id. */
static uint32_t find(const char *name, uint32_t id, dotnet_pal_account *out) {
    FILE *file = fopen("/etc/passwd", "r");
    if (!file) return errno == ENOENT ? DOTNET_PAL_NOT_FOUND : DOTNET_PAL_OS_ERROR;
    char *line = NULL; size_t size = 0; uint32_t status = DOTNET_PAL_NOT_FOUND;
    while (status == DOTNET_PAL_NOT_FOUND && getline(&line, &size, file) >= 0) {
        line[strcspn(line, "\n")] = 0;
        char *rest = line, *entry = field(&rest), *user, *group, *home, *shell; uint32_t uid, gid;
        (void)field(&rest); user = field(&rest); group = field(&rest); (void)field(&rest); home = field(&rest); shell = field(&rest);
        /* A line that is no entry (a comment, a compat line, one without its numbers) is skipped, as the C library skips it. */
        if (!entry || !*entry || *entry == '#' || *entry == '+' || *entry == '-' || !user || !group || !number(user, &uid) || !number(group, &gid)) continue;
        if (name ? strcmp(entry, name) != 0 : uid != id) continue;
        memset(out, 0, sizeof *out);
        out->user_id = uid; out->group_id = gid;
        status = put(out->name, sizeof out->name, entry) && put(out->home, sizeof out->home, home ? home : "") && put(out->shell, sizeof out->shell, shell ? shell : "")
            ? DOTNET_PAL_OK : DOTNET_PAL_OS_ERROR;
    }
    free(line); fclose(file);
    return status;
}
static void fill(dotnet_pal_account *out, uint32_t id, const char *name) {
    memset(out, 0, sizeof *out);
    out->user_id = id; out->group_id = 7;
    strcpy((char *)out->name, name); strcpy((char *)out->home, "/home"); strcpy((char *)out->shell, "/bin/sh");
}
/* A broken answer about an account, by the number of the call. */
static uint32_t broken_user(uint32_t asked, dotnet_pal_account *out, int step) {
    fill(out, asked, "pal");
    switch (step) {
    case 0: out->name[0] = 0; return DOTNET_PAL_OK;                                    /* an account without a name */
    case 1: memset(out->name, 'n', sizeof out->name); return DOTNET_PAL_OK;            /* no terminator in the name */
    case 2: memset(out->home, 'h', sizeof out->home); return DOTNET_PAL_OK;            /* no terminator in the home */
    case 3: memset(out->shell, 's', sizeof out->shell); return DOTNET_PAL_OK;          /* no terminator in the shell */
    case 4: out->user_id = asked + 1; return DOTNET_PAL_OK;                            /* another account than the one asked for */
    case 5: return 99u;                                                                /* no such status */
    case 6: return DOTNET_PAL_BUFFER_TOO_SMALL;                                        /* a status only the lists have */
    case 7: return DOTNET_PAL_TIMEOUT;                                                 /* a status of the kernel group */
    default: return DOTNET_PAL_NOT_FOUND;                                              /* outputs written by a failing call */
    }
}
static uint32_t user_by_id(uint32_t user_id, dotnet_pal_account *out, size_t out_size) {
    if (out_size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    if (pal_accounts_fault == 2) { static int step; return broken_user(user_id, out, step++); }
    return find(NULL, user_id, out);
}
static uint32_t user_by_name(const uint8_t *name, size_t name_length, dotnet_pal_account *out, size_t out_size) {
    if (out_size < sizeof *out) return DOTNET_PAL_INVALID_ARGUMENT;
    /* By name nothing says which id was asked for, so step 4 is an honest answer there: the test expects it to pass. */
    if (pal_accounts_fault == 2) { static int step; return broken_user(41, out, step++); }
    char text[LONGEST_NAME + 1];
    if (name_length == 0 || name_length > LONGEST_NAME || memchr(name, 0, name_length)) return DOTNET_PAL_INVALID_ARGUMENT;
    memcpy(text, name, name_length); text[name_length] = 0;
    return find(text, 0, out);
}
/* A broken answer about a list, by the number of the call. */
static uint32_t broken_groups(uint32_t *out, size_t capacity, size_t *count, int step) {
    for (size_t i = 0; i < capacity; ++i) out[i] = 1000 + (uint32_t)i;
    switch (step) {
    case 0: *count = MOST_GROUPS + 1; return DOTNET_PAL_OK;                            /* more groups than any list holds */
    case 1: *count = MOST_GROUPS + 1; return DOTNET_PAL_BUFFER_TOO_SMALL;              /* the same, as the length a longer buffer needs */
    case 2: *count = capacity + 3; return DOTNET_PAL_BUFFER_TOO_SMALL;                 /* a partial list together with BUFFER_TOO_SMALL */
    case 3: *count = capacity + 3; return DOTNET_PAL_OK;                               /* success with more groups than were written */
    case 4: *count = 1; return 99u;                                                    /* no such status */
    case 5: *count = 1; return DOTNET_PAL_ACCESS_DENIED;                               /* a status of the I/O groups */
    case 6: *count = capacity - 1; return DOTNET_PAL_BUFFER_TOO_SMALL;                 /* a list called too small for a buffer it fits in */
    default: *count = 1; return DOTNET_PAL_OS_ERROR;                                   /* outputs written by a failing call */
    }
}
static int held(const uint32_t *list, size_t count, uint32_t group) { while (count--) if (*list++ == group) return 1; return 0; }
/* The list as the contract wants it: the count either way, the ids only when all of them fit. */
static uint32_t answer(const uint32_t *list, size_t found, uint32_t *out, size_t capacity, size_t *count) {
    *count = found;
    if (found > capacity) return DOTNET_PAL_BUFFER_TOO_SMALL;
    if (found) memcpy(out, list, found * sizeof *list);
    return DOTNET_PAL_OK;
}
static uint32_t process_groups(uint32_t *out, size_t capacity, size_t *count) {
    if (pal_accounts_fault == 2) { static int step; return broken_groups(out, capacity, count, step++); }
    FILE *file = fopen("/proc/self/status", "r");
    if (!file) return errno == ENOENT ? DOTNET_PAL_UNSUPPORTED : DOTNET_PAL_OS_ERROR;
    uint32_t *list = malloc(MOST_GROUPS * sizeof *list);
    char *line = NULL; size_t size = 0, found = 0; uint32_t status = DOTNET_PAL_OS_ERROR;
    while (list && getline(&line, &size, file) >= 0) {
        if (strncmp(line, "Groups:", 7) != 0) continue;
        char *at = line + 7, *end;
        for (;;) {
            errno = 0;
            unsigned long long value = strtoull(at, &end, 10);
            if (end == at) break;
            if (errno || value > UINT32_MAX || found == MOST_GROUPS) { found = SIZE_MAX; break; }
            list[found++] = (uint32_t)value; at = end;
        }
        if (found != SIZE_MAX) status = answer(list, found, out, capacity, count);
        break;
    }
    if (!list) status = DOTNET_PAL_OUT_OF_MEMORY;
    free(line); free(list); fclose(file);
    return status;
}
static uint32_t user_groups(const uint8_t *name, size_t name_length, uint32_t primary_group, uint32_t *out, size_t capacity, size_t *count) {
    if (pal_accounts_fault == 2) { static int step; return broken_groups(out, capacity, count, step++); }
    char text[LONGEST_NAME + 1]; dotnet_pal_account account;
    if (name_length == 0 || name_length > LONGEST_NAME || memchr(name, 0, name_length)) return DOTNET_PAL_INVALID_ARGUMENT;
    memcpy(text, name, name_length); text[name_length] = 0;
    /* A name that is no account has no groups to list. An entry too long for the boundary is still an account. */
    uint32_t status = find(text, 0, &account);
    if (status != DOTNET_PAL_OK && status != DOTNET_PAL_OS_ERROR) return status;
    FILE *file = fopen("/etc/group", "r");
    if (!file && errno != ENOENT) return DOTNET_PAL_OS_ERROR;
    uint32_t *list = malloc(MOST_GROUPS * sizeof *list);
    if (!list) { if (file) fclose(file); return DOTNET_PAL_OUT_OF_MEMORY; }
    char *line = NULL; size_t size = 0, found = 0;
    list[found++] = primary_group;
    status = DOTNET_PAL_OK;
    while (file && getline(&line, &size, file) >= 0) {
        line[strcspn(line, "\n")] = 0;
        char *rest = line, *entry = field(&rest), *group, *members; uint32_t gid;
        (void)field(&rest); group = field(&rest); members = field(&rest);
        if (!entry || !*entry || *entry == '#' || *entry == '+' || *entry == '-' || !group || !number(group, &gid) || !members) continue;
        char *more = NULL;
        for (char *member = strtok_r(members, ",", &more); member; member = strtok_r(NULL, ",", &more)) {
            if (strcmp(member, text) != 0 || held(list, found, gid)) continue;
            if (found == MOST_GROUPS) { status = DOTNET_PAL_OS_ERROR; break; }
            list[found++] = gid;
        }
    }
    if (status == DOTNET_PAL_OK) status = answer(list, found, out, capacity, count);
    free(line); free(list); if (file) fclose(file);
    return status;
}
static const dotnet_pal_host_accounts table = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_accounts), DOTNET_PAL_CAP_ACCOUNTS},
    {user_by_id, user_by_name, process_groups, user_groups, NULL},
};
/* Every question is optional, so no missing callback rejects a table: a header that does not offer the group does. */
static const dotnet_pal_host_accounts malformed = {
    {DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_accounts), 0},
    {user_by_id, user_by_name, process_groups, user_groups, NULL},
};
static const dotnet_pal_host_accounts silent = {{DOTNET_PAL_ABI_VERSION, sizeof(dotnet_pal_host_accounts), DOTNET_PAL_CAP_ACCOUNTS}, {NULL, NULL, NULL, NULL, NULL}};
const dotnet_pal_host_accounts *dotnet_pal_host_accounts_v2(void) { return pal_accounts_fault == 1 ? &malformed : pal_accounts_fault == 3 ? &silent : &table; }
