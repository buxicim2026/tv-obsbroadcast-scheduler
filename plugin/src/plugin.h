/*
 * tv-obsbroadcast-scheduler — OBS plugin entry points.
 *
 * Registered:
 *   1. An Input source called "Broadcast Scheduler Control" — used as the
 *      "control plane" handle that holds the OBS-WS connection settings and
 *      the user's edit queue. Its properties panel is the primary UI surface.
 *   2. A custom Browser dock that loads http://127.0.0.1:8789/admin once the
 *      Rust engine is up.
 *
 * The plugin spawns and supervises a single Rust engine subprocess; if it
 * crashes, we respawn on a backoff. All work is non-blocking via OBS events.
 */

#ifndef TVBS_PLUGIN_H
#define TVBS_PLUGIN_H

#include <obs-module.h>
#include <util/platform.h>
#include <util/threading.h>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#endif

#include "engine_proc.h"
#include "source.h"
#include "source_properties.h"

#define TVBS_MODULE_NAME          "tv-obsbroadcast-scheduler"
#define TVBS_DEFAULT_BOOTSTRAP_PORT 8789

/* Global module state (single source + single engine per OBS process). */
extern struct tvbs_state {
    bool initialized;
    /* Source kind id we registered (input). */
    const char *source_id;
    /* The engine subprocess manager. */
    struct tvbs_engine_proc engine;
    /* True after the engine has responded to /healthz at least once. */
    bool engine_ready;
    /* Shared secret for /api/bootstrap. Issued lazily on first launch and
     * persisted into the source's settings. */
    char bootstrap_secret[64];
} tvbs_g_state;

/* Logging helpers (also used by submodules). */
#define TVBS_LOG_TAG "[TVBS] "
#define tvbs_info(fmt, ...)  blog(LOG_INFO,  TVBS_LOG_TAG fmt, ##__VA_ARGS__)
#define tvbs_warn(fmt, ...)  blog(LOG_WARNING, TVBS_LOG_TAG fmt, ##__VA_ARGS__)
#define tvbs_error(fmt, ...) blog(LOG_ERROR, TVBS_LOG_TAG fmt, ##__VA_ARGS__)
#define tvbs_debug(fmt, ...) blog(LOG_DEBUG, TVBS_LOG_TAG fmt, ##__VA_ARGS__)

#endif /* TVBS_PLUGIN_H */
