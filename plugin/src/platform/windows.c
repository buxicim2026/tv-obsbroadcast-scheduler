/*
 * platform/windows.c — Win32 spawning of the Rust engine using
 * CreateProcessA + a Job Object so OBS terminating cleanly kills the engine.
 */

#ifdef _WIN32

#include <windows.h>
#include <winsock2.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "platform.h"

static HANDLE g_job = NULL;

bool tvbs_platform_spawn_engine(const char *data_dir, tvbs_pid_t *out_pid)
{
    if (!data_dir || !out_pid)
        return false;

    char exe[1024];
    snprintf(exe, sizeof(exe), "%s\\engine\\tv-obsbroadcast-scheduler.exe",
             data_dir);
    if (GetFileAttributesA(exe) == INVALID_FILE_ATTRIBUTES) {
        blog(LOG_WARNING, TVBS_LOG_TAG "engine binary not found at %s", exe);
        return false;
    }

    char cmd[2048];
    snprintf(cmd, sizeof(cmd), "\"%s\" --audio-mode scheduler", exe);

    /* Tear off any existing job (cleanup on respawn). */
    if (g_job) {
        CloseHandle(g_job);
        g_job = NULL;
    }

    STARTUPINFOA si;
    PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof(si));
    si.cb = sizeof(si);
    memset(&pi, 0, sizeof(pi));

    if (!CreateProcessA(NULL, cmd, NULL, NULL, FALSE,
                        CREATE_NO_WINDOW, NULL, data_dir, &si, &pi)) {
        blog(LOG_WARNING, TVBS_LOG_TAG "CreateProcessA failed: %lu",
             GetLastError());
        return false;
    }

    g_job = CreateJobObjectA(NULL, NULL);
    if (g_job) {
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION info;
        memset(&info, 0, sizeof(info));
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(g_job,
                                JobObjectExtendedLimitInformation,
                                &info, sizeof(info));
        AssignProcessToJobObject(g_job, pi.hProcess);
    }

    CloseHandle(pi.hThread);
    *out_pid = pi.hProcess;
    return true;
}

void tvbs_platform_kill_engine(tvbs_pid_t pid)
{
    if (!pid)
        return;
    TerminateProcess(pid, 0);
    WaitForSingleObject(pid, 1000);
    CloseHandle(pid);
    if (g_job) {
        CloseHandle(g_job);
        g_job = NULL;
    }
}

bool tvbs_platform_is_alive(tvbs_pid_t pid)
{
    if (!pid)
        return false;
    return WaitForSingleObject(pid, 0) == WAIT_TIMEOUT;
}

#endif /* _WIN32 */
