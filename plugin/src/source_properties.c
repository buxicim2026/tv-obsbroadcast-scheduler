/*
 * source_properties.c — Qt properties panel for the Broadcast Scheduler
 * Control source.
 *
 * The panel is intentionally simple: the heavy editing happens in the
 * `admin/` web UI loaded via the dock. What stays in Qt are the bits that
 * are part of the source's persisted settings (so they travel with the scene):
 *   * OBS WebSocket connection (host / port / password / TLS)
 *   * Target Media Source name
 *   * Scheduler enable flag
 *   * Show / hide playlist editor toggle (loaded from the engine on demand)
 *
 * Every change is forwarded to the Rust engine via `/api/bootstrap` so a
 * fresh source instance in a fresh scene still has the right configuration.
 */

#include <obs-module.h>
#include <util/platform.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "plugin.h"
#include "source.h"
#include "source_properties.h"
#include "tcp_post.h"

#define tvbs_props_log_info(fmt, ...)  blog(LOG_INFO,  TVBS_LOG_TAG fmt, ##__VA_ARGS__)
#define tvbs_props_log_warn(fmt, ...)  blog(LOG_WARNING,TVBS_LOG_TAG fmt, ##__VA_ARGS__)

#define TVBS_DEFAULT_ENGINE_PORT 8789

/* --------------------------------------------------------------------------
 * Button: Test Connection
 * ------------------------------------------------------------------------ */
static bool test_connection_clicked(obs_properties_t *props,
                                    obs_property_t *property,
                                    void *data)
{
    UNUSED_PARAMETER(property);
    struct tvbs_source_data *sd = data;

    tvbs_tcp_init();

    char body[1024];
    int host_port = TVBS_DEFAULT_ENGINE_PORT;
    /* Pull current settings back from `data` (callback is invoked with the
     * `data` pointer matching source_info->data). */
    snprintf(body, sizeof(body),
        "{\"bootstrap_token\":\"%s\",\"host\":\"%s\",\"port\":%d,\"password\":\"%s\",\"tls\":%s,\"target_input\":\"%s\"}",
        tvbs_g_state.bootstrap_secret,
        sd->ws_host, sd->ws_port, sd->ws_password,
        sd->ws_tls ? "true" : "false",
        sd->target_input);

    int rc = tvbs_http_post_json("127.0.0.1", host_port, "/api/bootstrap", body);
    if (rc >= 200 && rc < 300) {
        tvbs_props_log_info("bootstrap POST ok: HTTP %d", rc);
    } else {
        tvbs_props_log_warn("bootstrap POST failed: HTTP %d (engine port %d running?)",
                            rc, host_port);
    }
    UNUSED_PARAMETER(props);
    return true;
}

/* --------------------------------------------------------------------------
 * Button: Open Admin in Browser
 * ------------------------------------------------------------------------ */
static bool open_admin_clicked(obs_properties_t *props,
                               obs_property_t *property,
                               void *data)
{
    UNUSED_PARAMETER(property);
    UNUSED_PARAMETER(data);
    UNUSED_PARAMETER(props);
    /* OBS 28+ has no platform-independent "open URL" helper, so we just
     * log the URL — users paste it into their browser, or use the dock. */
    tvbs_props_log_info("open this URL to edit the playlist: http://127.0.0.1:%d/admin",
                        TVBS_DEFAULT_ENGINE_PORT);
    return false;
}

/* --------------------------------------------------------------------------
 * Property panel construction
 * ------------------------------------------------------------------------ */
obs_properties_t *tvbs_source_build_properties(void *data)
{
    UNUSED_PARAMETER(data);

    obs_properties_t *p = obs_properties_create();

    /* --- OBS WebSocket connection --- */
    obs_properties_t *ws = obs_properties_create();
    obs_properties_add_text(ws, "ws_host", "Host", OBS_TEXT_DEFAULT);
    obs_properties_add_int(ws, "ws_port", "Port", 1, 65535, 1);
    obs_properties_add_text(ws, "ws_password", "Password", OBS_TEXT_PASSWORD);
    obs_properties_add_bool(ws, "ws_tls", "Use TLS (wss://)");
    obs_properties_add_button(ws, "test_connection", obs_module_text("TestConnection"),
                              test_connection_clicked);
    obs_properties_add_group(p, "ws_group", "OBS WebSocket (broadcast scheduler)",
                             OBS_GROUP_NORMAL, ws);

    /* --- Target --- */
    obs_properties_t *tgt = obs_properties_create();
    obs_properties_add_text(tgt, "target_input", "Media Source Name",
                            OBS_TEXT_DEFAULT);
    obs_properties_add_group(p, "tgt_group", "Target",
                             OBS_GROUP_NORMAL, tgt);

    /* --- Scheduler --- */
    obs_properties_t *sc = obs_properties_create();
    obs_properties_add_bool(sc, "scheduler_enabled", obs_module_text("Enabled"));
    obs_properties_add_group(p, "sc_group", "Scheduler",
                             OBS_GROUP_NORMAL, sc);

    /* --- Admin entry --- */
    obs_properties_t *ad = obs_properties_create();
    obs_properties_add_button(ad, "open_admin", "Open Admin in Browser",
                              open_admin_clicked);
    obs_properties_add_group(p, "ad_group", "Admin (advanced)",
                             OBS_GROUP_NORMAL, ad);

    /* --- Engine / status (read-only banner) --- */
    obs_properties_add_text(p, "engine_status",
                            obs_module_text("EngineStatus"),
                            OBS_TEXT_DEFAULT);

    return p;
}

/* --------------------------------------------------------------------------
 * Bootstrap push (called from source_create / source_update)
 * ------------------------------------------------------------------------ */

void tvbs_push_settings_to_engine(struct tvbs_source_data *data)
{
    if (!data) return;

    tvbs_tcp_init();

    char body[1024];
    snprintf(body, sizeof(body),
        "{\"bootstrap_token\":\"%s\","
        "\"host\":\"%s\",\"port\":%d,\"password\":\"%s\","
        "\"tls\":%s,\"target_input\":\"%s\","
        "\"scheduler_enabled\":%s}",
        tvbs_g_state.bootstrap_secret,
        data->ws_host ? data->ws_host : "127.0.0.1",
        data->ws_port ? data->ws_port : 4455,
        data->ws_password ? data->ws_password : "",
        data->ws_tls ? "true" : "false",
        data->target_input ? data->target_input : "main_media",
        "true");
    int rc = tvbs_http_post_json("127.0.0.1", TVBS_DEFAULT_ENGINE_PORT,
                                 "/api/bootstrap", body);
    if (rc < 0) {
        tvbs_props_log_warn("bootstrap post: engine not reachable "
                            "(will retry next time you edit settings)");
    } else if (rc >= 200 && rc < 300) {
        tvbs_props_log_info("bootstrap post ok (%d)", rc);
    } else {
        tvbs_props_log_warn("bootstrap post returned %d", rc);
    }
}
