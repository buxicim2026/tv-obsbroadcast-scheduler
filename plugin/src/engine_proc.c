/*
 * engine_proc.c — start / stop the Rust engine subprocess. Platform-specific
 * pieces live in src/platform/.
 */

#include <obs-module.h>
#include <util/platform.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#include <winsock2.h>
#include <windows.h>
#endif

#include "plugin.h"
#include "engine_proc.h"
#include "platform/platform.h"

bool tvbs_engine_proc_start(struct tvbs_engine_proc *e)
{
    if (!e)
        return false;
    if (e->running)
        return true;

    /* Resolve plugin data dir once. */
    if (e->data_dir[0] == '\0') {
        const char *d = obs_get_module_data_path(obs_current_module());
        if (d)
            snprintf(e->data_dir, sizeof(e->data_dir), "%s", d);
        else
            snprintf(e->data_dir, sizeof(e->data_dir), "%s", ".");
    }

    tvbs_pid_t pid;
    if (!tvbs_platform_spawn_engine(e->data_dir, &pid)) {
        tvbs_warn("failed to spawn engine");
        return false;
    }
    e->pid = pid;
    e->running = true;
    tvbs_info("engine subprocess spawned (pid=%p)", (void *)(intptr_t)pid);
    return true;
}

void tvbs_engine_proc_stop(struct tvbs_engine_proc *e)
{
    if (!e || !e->running)
        return;
    tvbs_platform_kill_engine(e->pid);
    e->pid = TVBS_INVALID_PID;
    e->running = false;
}

bool tvbs_engine_proc_alive(struct tvbs_engine_proc *e)
{
    if (!e || !e->running)
        return false;
    return tvbs_platform_is_alive(e->pid);
}

void tvbs_issue_bootstrap_secret(char *out, size_t out_len)
{
    if (!out || out_len < 17) {
        return;
    }
    /* 16 random bytes -> 32 hex chars. */
    unsigned char raw[16];
#ifdef _WIN32
    /* CryptGenRandom is overkill; RtlGenRandom / rand_s is fine for non-safety
     * use, but we'll just call rand() seeded from time for init-scaffold;
     * the `c-plugin-dock-engine` todo swaps to BCryptGenRandom on Win. */
    srand((unsigned)time(NULL) ^ (unsigned)GetCurrentProcessId());
    for (size_t i = 0; i < sizeof(raw); i++)
        raw[i] = (unsigned char)(rand() & 0xff);
#else
    srandom((unsigned long)time(NULL) ^ (unsigned long)getpid());
    for (size_t i = 0; i < sizeof(raw); i++)
        raw[i] = (unsigned char)(random() & 0xff);
#endif
    static const char *hx = "0123456789abcdef";
    for (size_t i = 0; i < sizeof(raw); i++) {
        out[i * 2 + 0] = hx[(raw[i] >> 4) & 0xf];
        out[i * 2 + 1] = hx[raw[i] & 0xf];
    }
    out[32] = '\0';
    (void)out_len;
}
