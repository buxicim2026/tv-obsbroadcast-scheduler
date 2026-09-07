/*
 * platform/bsd.c — macOS-specific spawning. Different fork-wait quirks; we
 * use posix_spawn + kill(-pid, SIGTERM) so the engine + any descendants
 * are terminated together.
 */

#ifdef __APPLE__

#include <errno.h>
#include <signal.h>
#include <spawn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "platform.h"
#include "../plugin.h" /* TVBS_LOG_TAG logging macro */

extern char **environ;

bool tvbs_platform_spawn_engine(const char *data_dir, tvbs_pid_t *out_pid)
{
    if (!data_dir || !out_pid)
        return false;

    char exe[1024];
    snprintf(exe, sizeof(exe), "%s/engine/tv-obsbroadcast-scheduler",
             data_dir);
    if (access(exe, X_OK) != 0) {
        blog(LOG_WARNING, TVBS_LOG_TAG "engine binary not executable: %s",
             exe);
        return false;
    }

    char *argv[] = { exe, "--audio-mode", "scheduler", NULL };
    pid_t pid;
    int rc = posix_spawn(&pid, exe, NULL, NULL, argv, environ);
    if (rc != 0) {
        blog(LOG_WARNING, TVBS_LOG_TAG "posix_spawn failed: %s",
             strerror(rc));
        return false;
    }
    *out_pid = pid;
    return true;
}

void tvbs_platform_kill_engine(tvbs_pid_t pid)
{
    if (pid <= 0)
        return;
    kill(pid, SIGTERM);
    for (int i = 0; i < 20; i++) {
        if (waitpid(pid, NULL, WNOHANG) == pid)
            return;
        usleep(50 * 1000);
    }
    kill(pid, SIGKILL);
    waitpid(pid, NULL, 0);
}

bool tvbs_platform_is_alive(tvbs_pid_t pid)
{
    if (pid <= 0)
        return false;
    int st;
    pid_t r = waitpid(pid, &st, WNOHANG);
    if (r == 0)
        return true;
    return false;
}

#endif /* __APPLE__ */
