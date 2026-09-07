/*
 * platform.h — OS-specific engine subprocess API.
 *
 * Implementations:
 *   platform/windows.c  — CreateProcessA + JobObject
 *   platform/posix.c    — fork + exec (Linux, others)
 *   platform/bsd.c      — posix_spawn + process group (macOS)
 */

#ifndef TVBS_PLATFORM_H
#define TVBS_PLATFORM_H

#include "../engine_proc.h"

bool tvbs_platform_spawn_engine(const char *data_dir, tvbs_pid_t *out_pid);
void tvbs_platform_kill_engine(tvbs_pid_t pid);
bool tvbs_platform_is_alive(tvbs_pid_t pid);

#endif
