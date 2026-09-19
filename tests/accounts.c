/* Conformance test of the accounts group on Linux: every answer is checked against
 * what the C library says about the same databases and the same process. */
#define _GNU_SOURCE
#include "dotnet_pal.h"
#include <assert.h>
#include <grp.h>
#include <pthread.h>
#include <pwd.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>
#ifdef PAL_HOST_TEST
extern int pal_accounts_fault;
#endif
#define INVALID DOTNET_PAL_INVALID_ARGUMENT
#define SMALL DOTNET_PAL_BUFFER_TOO_SMALL
#define MISSING DOTNET_PAL_NOT_FOUND
/* The limits of the front end, which the header does not name. */
#define LONGEST_NAME 255
#define MOST_GROUPS 65536
#define GUARD UINT32_C(0xAAAAAAAA)
#define ROUNDS 50
static const dotnet_pal_accounts_ops *a;
/* What the C library says about one account, copied out of its static storage. `carried` is 0 for an entry the boundary's fields cannot hold. */
struct known { uint32_t uid, gid; int carried; size_t groups; char name[256], home[1024], shell[256]; };
struct tally { uint64_t user_ok, groups_ok, failed; };
static struct tally expected;
static struct known accounts[4096];
static size_t known_accounts;
static uint32_t list[MOST_GROUPS + 2], theirs[MOST_GROUPS];

static int zero(const void *bytes, size_t size) { for (const uint8_t *byte = bytes; size--; ++byte) if (*byte) return 0; return 1; }
static int ascending(const void *left, const void *right) { uint32_t l = *(const uint32_t *)left, r = *(const uint32_t *)right; return (l > r) - (l < r); }
static void count_user(struct tally *tally, uint32_t status) { if (status == 0) ++tally->user_ok; else ++tally->failed; }
static void count_list(struct tally *tally, uint32_t status) { if (status == 0) ++tally->groups_ok; else ++tally->failed; }
static void copy(struct known *to, const struct passwd *from) {
    memset(to, 0, sizeof *to);
    to->uid = from->pw_uid; to->gid = from->pw_gid;
    to->carried = strlen(from->pw_name) < sizeof to->name && strlen(from->pw_dir) < sizeof to->home && strlen(from->pw_shell) < sizeof to->shell;
    if (to->carried) { strcpy(to->name, from->pw_name); strcpy(to->home, from->pw_dir); strcpy(to->shell, from->pw_shell); }
}
/* The answer is the account: its ids, its three texts, and nothing but zeros behind each text. */
static int same(const dotnet_pal_account *got, const struct known *want) {
    return got->user_id == want->uid && got->group_id == want->gid && memcmp(got->name, want->name, sizeof got->name) == 0
        && memcmp(got->home, want->home, sizeof got->home) == 0 && memcmp(got->shell, want->shell, sizeof got->shell) == 0;
}
/* One lookup by id (name NULL) or by the `length` bytes at `name`: the account `want`, or `status` and a cleared answer. */
static void user(struct tally *tally, uint32_t id, const char *name, size_t length, const struct known *want, uint32_t status) {
    dotnet_pal_account got;
    memset(&got, 0xAA, sizeof got);
    uint32_t result = name ? a->user_by_name((const uint8_t *)name, length, &got, sizeof got) : a->user_by_id(id, &got, sizeof got);
    count_user(tally, result);
    if (want && want->carried) assert(result == 0 && same(&got, want));
    else assert(result == (want ? DOTNET_PAL_OS_ERROR : status) && zero(&got, sizeof got));
}
/* One list, of this process (name NULL) or of an account, into `capacity` entries behind which a guard stands. */
static uint32_t ask(struct tally *tally, const char *name, size_t length, uint32_t primary, size_t capacity, size_t *count) {
    for (size_t i = 0; i <= capacity; ++i) list[i] = GUARD;
    *count = 7;
    uint32_t status = name ? a->user_groups((const uint8_t *)name, length, primary, capacity ? list : NULL, capacity, count) : a->process_groups(capacity ? list : NULL, capacity, count);
    count_list(tally, status);
    assert(list[capacity] == GUARD);
    return status;
}
/* The list is `theirs` in any order where it fits; its length alone, and a cleared buffer, where it does not. */
static void lists(struct tally *tally, const char *name, uint32_t primary, size_t want) {
    size_t count, length = name ? strlen(name) : 0;
    qsort(theirs, want, sizeof *theirs, ascending);
    size_t capacities[] = {want, want + 5, MOST_GROUPS};
    for (size_t i = 0; i < 3; ++i) {
        assert(ask(tally, name, length, primary, capacities[i], &count) == 0 && count == want);
        qsort(list, want, sizeof *list, ascending);
        assert(memcmp(list, theirs, want * sizeof *list) == 0);
    }
    if (want == 0) return;
    assert(ask(tally, name, length, primary, want - 1, &count) == SMALL && count == want && zero(list, (want - 1) * sizeof *list));
    assert(ask(tally, name, length, primary, 0, &count) == SMALL && count == want);
}
static void process_list(struct tally *tally) {
    int want = getgroups(MOST_GROUPS, theirs);
    assert(want >= 0);
    lists(tally, NULL, 0, (size_t)want);
}
static size_t user_list(struct tally *tally, const char *name, uint32_t primary) {
    int want = MOST_GROUPS;
    assert(getgrouplist(name, primary, theirs, &want) >= 0 && want >= 1);
    lists(tally, name, primary, (size_t)want);
    return (size_t)want;
}
/* As root: the groups of a process that has just been given a known list, and of one that has none. */
static int regrouped(void) {
    static const gid_t given[] = {54321, 7, 65534, 12};
    struct tally tally = {0, 0, 0}; dotnet_pal_accounts_stats before, after; size_t count;
    assert(a->read_stats(&before, sizeof before) == 0);
    if (setgroups(4, given) != 0) return 77;
    assert(getgroups(0, NULL) == 4);
    process_list(&tally);
    assert(ask(&tally, NULL, 0, 0, 3, &count) == SMALL && count == 4 && zero(list, 3 * sizeof *list));
    assert(ask(&tally, NULL, 0, 0, 0, &count) == SMALL && count == 4);
    if (setgroups(0, NULL) != 0) return 77;
    /* No groups at all is an answer, and it fits into no buffer at all. */
    assert(ask(&tally, NULL, 0, 0, 0, &count) == 0 && count == 0);
    assert(ask(&tally, NULL, 0, 0, 4, &count) == 0 && count == 0);
    assert(a->read_stats(&after, sizeof after) == 0 && after.user_ok == before.user_ok);
    assert(after.groups_ok == before.groups_ok + tally.groups_ok && after.rejected_or_failed == before.rejected_or_failed + tally.failed);
    return 0;
}
/* Every known account by id, by name and its groups, again and again, while another thread does the same. */
static void *again(void *argument) {
    struct tally *tally = argument;
    static _Thread_local uint32_t mine[64];
    for (int round = 0; round < ROUNDS; ++round) {
        for (size_t i = 0; i < known_accounts; ++i) {
            const struct known *want = &accounts[i];
            if (!want->carried) continue;
            dotnet_pal_account got; size_t count = 7;
            uint32_t status = a->user_by_name((const uint8_t *)want->name, strlen(want->name), &got, sizeof got);
            count_user(tally, status);
            assert(status == 0 && strcmp((const char *)got.name, want->name) == 0);
            status = a->user_by_id(got.user_id, &got, sizeof got);
            count_user(tally, status);
            assert(status == 0 && got.user_id == want->uid);
            status = a->user_groups((const uint8_t *)want->name, strlen(want->name), want->gid, mine, 64, &count);
            count_list(tally, status);
            assert(status == (want->groups > 64 ? SMALL : 0) && count == want->groups);
        }
    }
    return NULL;
}
int main(int argc, char **argv) {
#ifdef PAL_HOST_TEST
    pal_accounts_fault = argc > 1 ? atoi(argv[1]) : 0;
#else
    (void)argc; (void)argv;
#endif
    const dotnet_pal_api *api = dotnet_pal_get_api(2);
#ifdef PAL_HOST_TEST
    if (pal_accounts_fault == 1) { assert(!api); puts("ACCOUNTS malformed host rejected"); return 0; }
#endif
    assert(api && api->header.struct_size >= DOTNET_PAL_ACCOUNTS_API_SIZE);
    assert(api->header.capabilities & DOTNET_PAL_CAP_ACCOUNTS);
    a = &api->accounts;
    assert(a->user_by_id && a->user_by_name && a->process_groups && a->user_groups && a->read_stats);
    dotnet_pal_accounts_stats stats; dotnet_pal_account got; size_t count;
#ifdef PAL_HOST_TEST
    if (pal_accounts_fault == 2) {
        /* An account without a name, with a text that does not end, with another id than the one asked for or behind an unknown status is refused and cleared. */
        for (int i = 0; i < 8; ++i) user(&expected, 40, NULL, 0, NULL, DOTNET_PAL_OS_ERROR);
        user(&expected, 40, NULL, 0, NULL, MISSING); /* no such account is the lookup's to report, without the outputs of the call that reported it */
        for (int i = 0; i < 8; ++i) {
            memset(&got, 0xAA, sizeof got);
            uint32_t status = a->user_by_name((const uint8_t *)"pal", 3, &got, sizeof got);
            /* By name nothing says which id was asked for: the fifth answer is an account like any other. */
            if (i == 4) assert(status == 0 && got.user_id == 42 && strcmp((const char *)got.name, "pal") == 0);
            else assert(status == DOTNET_PAL_OS_ERROR && zero(&got, sizeof got));
        }
        user(&expected, 0, "pal", 3, NULL, MISSING);
        /* A count no list has, a partial list, success with more groups than fit, an unknown status, a list called too small for the
         * buffer it fits in: the count of a list that does not fit, or nothing. */
        for (int which = 0; which < 2; ++which) {
            const char *name = which ? "pal" : NULL;
            for (int i = 0; i < 8; ++i) {
                uint32_t status = ask(&expected, name, 3, 7, 4, &count);
                if (i == 2 || i == 3) assert(status == SMALL && count == 7); else assert(status == DOTNET_PAL_OS_ERROR && count == 0);
                assert(zero(list, 4 * sizeof *list));
            }
        }
        assert(a->read_stats(&stats, sizeof stats) == 0 && stats.user_ok == 1 && stats.groups_ok == 0 && stats.rejected_or_failed == 9 + 7 + 1 + 16);
        puts("ACCOUNTS host errors sanitized"); return 0;
    }
    if (pal_accounts_fault == 3) {
        /* A host answers the questions it can: each of the others is UNSUPPORTED, and the group is still there. */
        user(&expected, 0, NULL, 0, NULL, DOTNET_PAL_UNSUPPORTED);
        user(&expected, 0, "root", 4, NULL, DOTNET_PAL_UNSUPPORTED);
        assert(ask(&expected, NULL, 0, 0, 4, &count) == DOTNET_PAL_UNSUPPORTED && count == 0 && zero(list, 4 * sizeof *list));
        assert(ask(&expected, "root", 4, 0, 4, &count) == DOTNET_PAL_UNSUPPORTED && count == 0 && zero(list, 4 * sizeof *list));
        assert(a->read_stats(&stats, sizeof stats) == 0 && stats.user_ok == 0 && stats.groups_ok == 0 && stats.rejected_or_failed == 4);
        puts("ACCOUNTS absent host callbacks unsupported"); return 0;
    }
#endif
    /* Every account the C library enumerates, by its id and by its name. Two entries may share an id: by id the C library's own lookup says which one answers. */
    struct passwd *entry;
    setpwent();
    while ((entry = getpwent()) != NULL) { assert(known_accounts < sizeof accounts / sizeof *accounts); copy(&accounts[known_accounts++], entry); }
    endpwent();
    assert(known_accounts >= 1);
    size_t too_long = 0;
    for (size_t i = 0; i < known_accounts; ++i) {
        struct known by_id, by_name;
        assert((entry = getpwuid(accounts[i].uid)) != NULL); copy(&by_id, entry);
        user(&expected, accounts[i].uid, NULL, 0, &by_id, 0);
        if (!accounts[i].carried) { ++too_long; continue; }
        assert((entry = getpwnam(accounts[i].name)) != NULL); copy(&by_name, entry);
        user(&expected, 0, accounts[i].name, strlen(accounts[i].name), &by_name, 0);
        accounts[i].groups = user_list(&expected, accounts[i].name, accounts[i].gid);
    }
    /* Root, the user this process runs as, and nobody. */
    struct known root, self, nobody; int has_self = 0, has_nobody = 0;
    assert((entry = getpwuid(0)) != NULL); copy(&root, entry);
    user(&expected, 0, NULL, 0, &root, 0);
    user(&expected, 0, root.name, strlen(root.name), &root, 0);
    if ((entry = getpwuid(geteuid())) != NULL) { has_self = 1; copy(&self, entry); user(&expected, geteuid(), NULL, 0, &self, 0); }
    else user(&expected, geteuid(), NULL, 0, NULL, MISSING);
    if ((entry = getpwnam("nobody")) != NULL) {
        has_nobody = 1; copy(&nobody, entry);
        user(&expected, 0, "nobody", 6, &nobody, 0);
        assert((entry = getpwuid(nobody.uid)) != NULL); copy(&nobody, entry);
        user(&expected, nobody.uid, NULL, 0, &nobody, 0);
    }
    /* The name has a length and no terminator: what stands behind it is not part of it. */
    char behind[16] = "rootXYZ";
    memcpy(behind, root.name, strlen(root.name) < 8 ? strlen(root.name) : 0);
    if (strlen(root.name) < 8) { behind[strlen(root.name)] = 'X'; user(&expected, 0, behind, strlen(root.name), &root, 0); }

    /* An id and a name nobody has; the longest name there can be reaches the provider, which knows nobody by it. */
    uint32_t unused = 54321;
    while (getpwuid(unused)) ++unused;
    char longest[LONGEST_NAME + 2];
    memset(longest, 'n', sizeof longest); longest[LONGEST_NAME] = 0;
    assert(!getpwnam("pal-no-such-account") && !getpwnam(longest));
    user(&expected, unused, NULL, 0, NULL, MISSING);
    user(&expected, UINT32_MAX, NULL, 0, NULL, getpwuid(UINT32_MAX) ? 0 : MISSING);
    user(&expected, 0, "pal-no-such-account", 19, NULL, MISSING);
    user(&expected, 0, longest, LONGEST_NAME, NULL, MISSING);
    /* Names that are none: empty, one byte longer than the longest, with a NUL inside, not there at all. */
    user(&expected, 0, "", 0, NULL, INVALID);
    user(&expected, 0, longest, LONGEST_NAME + 1, NULL, INVALID);
    user(&expected, 0, "ro\0ot", 5, NULL, INVALID);
    user(&expected, 0, "root", SIZE_MAX, NULL, INVALID);
    memset(&got, 0xAA, sizeof got);
    assert(a->user_by_name(NULL, 4, &got, sizeof got) == INVALID && zero(&got, sizeof got)); count_user(&expected, INVALID);
    /* An answer that has no room is refused before anything is written. */
    memset(&got, 0xAA, sizeof got);
    assert(a->user_by_id(0, &got, sizeof got - 1) == INVALID && a->user_by_name((const uint8_t *)"root", 4, &got, sizeof got - 1) == INVALID && got.user_id == GUARD && got.name[0] == 0xAA);
    assert(a->user_by_id(0, NULL, sizeof got) == INVALID && a->user_by_name((const uint8_t *)"root", 4, NULL, sizeof got) == INVALID);
    assert(a->user_by_id(0, (dotnet_pal_account *)((uint8_t *)&got + 1), sizeof got) == INVALID);
    expected.failed += 5;

    /* The groups of this process, and as root those of a child that was given a list to report. */
    process_list(&expected);
    int regroup = -1;
    if (geteuid() == 0) {
        pid_t child = fork(); assert(child >= 0);
        if (child == 0) _exit(regrouped());
        int result = 0;
        assert(waitpid(child, &result, 0) == child && WIFEXITED(result) && (WEXITSTATUS(result) == 0 || WEXITSTATUS(result) == 77));
        regroup = WEXITSTATUS(result) == 0;
    }
    if (regroup != 1) puts("ACCOUNTS note: not root, or setgroups refused: the groups of a process with a list of the test's choosing were not checked");

    /* The groups of root, and of the account /etc/group names most often: with its own primary group, with another one, and with one it is a member of. */
    user_list(&expected, root.name, root.gid);
    user_list(&expected, root.name, 4242);
    char busiest[256] = ""; size_t most = 0;
    for (size_t i = 0; i < known_accounts; ++i) {
        FILE *file = fopen("/etc/group", "r"); char *line = NULL, *more; size_t size = 0, memberships = 0;
        if (!file || !accounts[i].carried) { if (file) fclose(file); continue; }
        while (getline(&line, &size, file) >= 0) {
            char *members = strrchr(line, ':'); int listed = 0;
            for (char *at = members ? strtok_r(members + 1, ",\n", &more) : NULL; at; at = strtok_r(NULL, ",\n", &more)) if (strcmp(at, accounts[i].name) == 0) listed = 1;
            memberships += (size_t)listed;
        }
        free(line); fclose(file);
        if (memberships > most) { most = memberships; strcpy(busiest, accounts[i].name); }
    }
    if (most >= 2) {
        assert((entry = getpwnam(busiest)) != NULL);
        uint32_t primary = entry->pw_gid; int want = MOST_GROUPS;
        user_list(&expected, busiest, primary);
        user_list(&expected, busiest, 4242);
        assert(getgrouplist(busiest, primary, theirs, &want) >= 0 && (size_t)want >= most);
        /* One of its supplementary groups as the primary one is still listed once. */
        uint32_t member_of = theirs[0] == primary ? theirs[1] : theirs[0];
        user_list(&expected, busiest, member_of);
    } else puts("ACCOUNTS note: /etc/group has no account in two groups: a list of several groups was not checked");
    /* The name of a list has a length as well. */
    if (strlen(root.name) < 8) {
        int want = MOST_GROUPS;
        assert(getgrouplist(root.name, root.gid, theirs, &want) >= 0);
        assert(ask(&expected, behind, strlen(root.name), root.gid, MOST_GROUPS, &count) == 0 && count == (size_t)want);
    }
    /* A name that is no account has no groups, whatever getgrouplist makes of it; names that are none; lists that have no room. */
    assert(ask(&expected, "pal-no-such-account", 19, 4242, 8, &count) == MISSING && count == 0 && zero(list, 8 * sizeof *list));
    assert(ask(&expected, longest, LONGEST_NAME, 4242, 8, &count) == MISSING && count == 0 && zero(list, 8 * sizeof *list));
    assert(ask(&expected, "", 0, 0, 8, &count) == INVALID && count == 0);
    assert(ask(&expected, longest, LONGEST_NAME + 1, 0, 8, &count) == INVALID && count == 0);
    assert(ask(&expected, "ro\0ot", 5, 0, 8, &count) == INVALID && count == 0);
    count = 7;
    assert(a->user_groups(NULL, 4, 0, list, 8, &count) == INVALID && count == 0);
    assert(a->user_groups((const uint8_t *)"root", 4, 0, list, 8, NULL) == INVALID && a->process_groups(list, 8, NULL) == INVALID);
    count = 7;
    assert(a->user_groups((const uint8_t *)"root", 4, 0, NULL, 8, &count) == INVALID && count == 0);
    count = 7;
    assert(a->process_groups(NULL, 8, &count) == INVALID && count == 0);
    count = 7;
    assert(a->process_groups((uint32_t *)((uint8_t *)list + 1), 8, &count) == INVALID && count == 0);
    count = 7;
    assert(a->process_groups(list, MOST_GROUPS + 1, &count) == INVALID && count == 0 && a->user_groups((const uint8_t *)"root", 4, 0, list, MOST_GROUPS + 1, &count) == INVALID);
    expected.failed += 8;
    assert(a->read_stats(NULL, sizeof stats) == INVALID && a->read_stats(&stats, sizeof stats - 1) == INVALID);

    /* Two threads ask at once. */
    pthread_t threads[2]; struct tally tallies[2] = {{0, 0, 0}, {0, 0, 0}};
    for (int i = 0; i < 2; ++i) assert(pthread_create(&threads[i], NULL, again, &tallies[i]) == 0);
    for (int i = 0; i < 2; ++i) {
        assert(pthread_join(threads[i], NULL) == 0 && tallies[i].user_ok == 2 * ROUNDS * (known_accounts - too_long));
        expected.user_ok += tallies[i].user_ok; expected.groups_ok += tallies[i].groups_ok; expected.failed += tallies[i].failed;
    }
    /* Counters: one for the accounts answered, one for the lists answered, one for everything refused. */
    assert(a->read_stats(&stats, sizeof stats) == 0);
    assert(stats.user_ok == expected.user_ok && stats.groups_ok == expected.groups_ok && stats.rejected_or_failed == expected.failed);
    printf("ACCOUNTS PASS accounts=%zu too_long=%zu self=%s nobody=%d regrouped=%d busiest=%s in %zu groups user_ok=%llu groups_ok=%llu refused=%llu\n", known_accounts, too_long,
        has_self ? self.name : "(none)", has_nobody, regroup, most >= 2 ? busiest : "(none)", most, (unsigned long long)stats.user_ok, (unsigned long long)stats.groups_ok, (unsigned long long)stats.rejected_or_failed);
    return 0;
}
