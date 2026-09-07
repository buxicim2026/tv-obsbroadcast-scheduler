/*
 * tv-obsbroadcast-scheduler — OBS plugin module entry.
 */

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#endif

#include <obs-module.h>
#include <util/platform.h>
#include <util/threading.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#include <windows.h>
#else
#include <pthread.h>
#include <unistd.h>
#endif

#include "plugin.h"
#include "platform/platform.h"

struct tvbs_state tvbs_g_state = {0};

/* --------------------------------------------------------------------------
 * Engine monitor thread
 * ------------------------------------------------------------------------ *
 * Polls once a second; respawns the engine subprocess if it has died
 * (e.g. the Rust process crashed at startup because the OBS plugin's data
 * dir wasn't ready). The thread is owned by obs_module_load and joined in
 * obs_module_unload.
 */

static volatile bool g_monitor_should_exit = false;

#ifdef _WIN32
static HANDLE g_monitor_thread = NULL;

static DWORD WINAPI engine_monitor_proc(LPVOID param)
{
    UNUSED_PARAMETER(param);
    while (!g_monitor_should_exit) {
        if (!tvbs_engine_proc_alive(&tvbs_g_state.engine)) {
            blog(LOG_WARNING, TVBS_LOG_TAG "engine not running; "
                                          "respawning");
            tvbs_engine_proc_stop(&tvbs_g_state.engine);
            tvbs_engine_proc_start(&tvbs_g_state.engine);
        }
        Sleep(1000);
    }
    return 0;
}
#else
static pthread_t g_monitor_thread;

static void *engine_monitor_proc(void *param)
{
    UNUSED_PARAMETER(param);
    while (!g_monitor_should_exit) {
        if (!tvbs_engine_proc_alive(&tvbs_g_state.engine)) {
            blog(LOG_WARNING, TVBS_LOG_TAG "engine not running; "
                                          "respawning");
            tvbs_engine_proc_stop(&tvbs_g_state.engine);
            tvbs_engine_proc_start(&tvbs_g_state.engine);
        }
        usleep(1000 * 1000); /* 1s */
    }
    return NULL;
}
#endif

static bool start_engine_monitor(void)
{
    g_monitor_should_exit = false;
#ifdef _WIN32
    g_monitor_thread = CreateThread(NULL, 0, engine_monitor_proc,
                                    NULL, 0, NULL);
    return g_monitor_thread != NULL;
#else
    return pthread_create(&g_monitor_thread, NULL,
                          engine_monitor_proc, NULL) == 0;
#endif
}

static void stop_engine_monitor(void)
{
    g_monitor_should_exit = true;
#ifdef _WIN32
    if (g_monitor_thread) {
        WaitForSingleObject(g_monitor_thread, 2000);
        CloseHandle(g_monitor_thread);
        g_monitor_thread = NULL;
    }
#else
    pthread_join(g_monitor_thread, NULL);
#endif
}

/* --------------------------------------------------------------------------
 * Plugin lifecycle
 * ------------------------------------------------------------------------ */

bool obs_module_load(void)
{
    tvbs_g_state.initialized = true;
    tvbs_g_state.source_id = "tvbs_control_source";

    /* Issue a fresh bootstrap secret if we don't have one cached. */
    if (tvbs_g_state.bootstrap_secret[0] == '\0') {
        tvbs_issue_bootstrap_secret(tvbs_g_state.bootstrap_secret,
                                    sizeof(tvbs_g_state.bootstrap_secret));
    }

    /* Spawn the Rust engine subprocess. */
    if (!tvbs_engine_proc_start(&tvbs_g_state.engine)) {
        blog(LOG_WARNING, TVBS_LOG_TAG "engine failed to spawn at startup; "
                  "monitor will retry");
    }

    /* Register the input source kind. */
    tvbs_source_register();

    /* Start the monitor thread so a later crash gets a respawn. */
    if (!start_engine_monitor()) {
        blog(LOG_WARNING, TVBS_LOG_TAG "failed to start engine monitor");
    }

    tvbs_info("tv-obsbroadcast-scheduler plugin loaded (v0.1.0)");
    return true;
}

void obs_module_unload(void)
{
    /* Tell the monitor thread to stop BEFORE we tear the engine down. */
    stop_engine_monitor();

    tvbs_source_unregister();
    tvbs_engine_proc_stop(&tvbs_g_state.engine);
    tvbs_g_state.initialized = false;
    tvbs_info("tv-obsbroadcast-scheduler plugin unloaded");
}

OBS_DECLARE_MODULE()
OBS_MODULE_USE_DEFAULT_LOCALE(TVBS_MODULE_NAME, "en-US")
