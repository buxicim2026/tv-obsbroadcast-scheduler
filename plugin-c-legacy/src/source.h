/*
 * source.h — registers the Broadcast Scheduler Control input source kind.
 *
 * The source has no actual audio/video output (it's a "control plane"
 * marker). Its observable side effects are:
 *   1. Storing obs-websocket connection settings + the bootstrap secret as
 *      source settings (persisted by OBS).
 *   2. Forwarding settings to the Rust engine via /api/bootstrap when the
 *      user edits them.
 *
 * The user's actual video plays through a separate "Media Source" in the
 * scene whose name is in `target_input`.
 */

#ifndef TVBS_SOURCE_H
#define TVBS_SOURCE_H

#include <stdbool.h>
#include <obs-module.h>

/* --------------------------------------------------------------------------
 * Per-source private data.
 *
 * Defined in the header (not privately in source.c) because the properties
 * panel (source_properties.c) also needs to read these fields to forward
 * settings to the engine via /api/bootstrap.
 * ------------------------------------------------------------------------ */
struct tvbs_source_data {
    /* Cached copies of the latest settings; used to detect changes that
     * need forwarding to the engine. */
    char *target_input;
    char *ws_host;
    int   ws_port;
    char *ws_password;
    bool  ws_tls;
    bool  scheduler_enabled;
    char *bootstrap_secret;  /* shared secret issued by the plugin */
};

/* Register the input source kind with libobs. */
void tvbs_source_register(void);

/* Unregister (called from obs_module_unload). */
void tvbs_source_unregister(void);

/* The registered id (matches tvbs_source_id constant used in plugin.c). */
#define TVBS_SOURCE_ID "tvbs_control_source"

#endif
