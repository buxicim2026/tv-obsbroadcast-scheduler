/*
 * platform/posix.c — Linux / general POSIX spawning of the Rust engine.
 */

#ifndef _WIN32

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include "platform.h"
#include "../plugin.h" /* TVBS_LOG_TAG logging macro */

#ifndef __APPLE__
/* macOS uses posix_spawn; see platform/bsd.c. */

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

    pid_t pid = fork();
    if (pid < 0) {
        blog(LOG_WARNING, TVBS_LOG_TAG "fork failed: %s", strerror(errno));
        return false;
    }
    if (pid == 0) {
        /* child: become a session leader so the engine survives any
         * intermediate ctrl-C. */
        setsid();
        execl(exe, exe, "--audio-mode", "scheduler", (char *)NULL);
        _exit(127);
    }

    /* parent */
    *out_pid = pid;
    return true;
}

void tvbs_platform_kill_engine(tvbs_pid_t pid)
{
    if (pid <= 0)
        return;
    kill(pid, SIGTERM);
    /* Wait briefly so we don't leak zombies. */
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

#endif /* !__APPLE__ */
#endif /* !_WIN32 */
