/*
 * engine_proc.h — supervises the Rust engine subprocess.
 */

#ifndef TVBS_ENGINE_PROC_H
#define TVBS_ENGINE_PROC_H

#include <obs-module.h>
#include <util/platform.h>

#ifdef _WIN32
#include <windows.h>
typedef HANDLE tvbs_pid_t;
#define TVBS_INVALID_PID NULL
#else
#include <sys/types.h>
typedef pid_t tvbs_pid_t;
#define TVBS_INVALID_PID (-1)
#endif

struct tvbs_engine_proc {
    tvbs_pid_t pid;
    bool       running;
    char       data_dir[1024]; /* resolved plugin data dir */
};

bool tvbs_engine_proc_start(struct tvbs_engine_proc *e);
void tvbs_engine_proc_stop(struct tvbs_engine_proc *e);
bool tvbs_engine_proc_alive(struct tvbs_engine_proc *e);

/* Utility: generate a 32-byte hex shared secret for /api/bootstrap. */
void tvbs_issue_bootstrap_secret(char *out, size_t out_len);

#endif
