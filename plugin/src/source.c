/*
 * source.c — input kind registration for "Broadcast Scheduler Control".
 *
 * The "render" path is a no-op; the source doesn't produce frames. It only
 * exists so the user can use OBS's native properties panel for the core UI
 * and so settings are persisted next to the scene.
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

/* --------------------------------------------------------------------------
 * Per-source private data
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

static const char *source_get_name(void *unused)
{
    UNUSED_PARAMETER(unused);
    return obs_module_text("SchedulerControl");
}

static void source_defaults(obs_data_t *s)
{
    obs_data_set_default_string(s, "target_input", "main_media");
    obs_data_set_default_string(s, "ws_host", "127.0.0.1");
    obs_data_set_default_int(s, "ws_port", 4455);
    obs_data_set_default_string(s, "ws_password", "");
    obs_data_set_default_bool(s, "ws_tls", false);
    obs_data_set_default_bool(s, "scheduler_enabled", false);
    obs_data_set_default_string(s, "bootstrap_secret", "");
}

static obs_properties_t *source_get_properties(void *data)
{
    return tvbs_source_build_properties(data);
}

static void source_clear_cache(struct tvbs_source_data *sd)
{
    if (!sd) return;
    bfree(sd->target_input);   sd->target_input = NULL;
    bfree(sd->ws_host);        sd->ws_host = NULL;
    bfree(sd->ws_password);    sd->ws_password = NULL;
    bfree(sd->bootstrap_secret); sd->bootstrap_secret = NULL;
}

static void *source_create(obs_data_t *settings, obs_source_t *source)
{
    struct tvbs_source_data *sd = bzalloc(sizeof(*sd));
    sd->target_input      = bstrdup(obs_data_get_string(settings, "target_input"));
    sd->ws_host           = bstrdup(obs_data_get_string(settings, "ws_host"));
    sd->ws_port           = (int)obs_data_get_int(settings, "ws_port");
    sd->ws_password       = bstrdup(obs_data_get_string(settings, "ws_password"));
    sd->ws_tls            = obs_data_get_bool(settings, "ws_tls");
    sd->scheduler_enabled = obs_data_get_bool(settings, "scheduler_enabled");

    const char *existing_secret = obs_data_get_string(settings, "bootstrap_secret");
    if (!existing_secret || !existing_secret[0]) {
        sd->bootstrap_secret = bstrdup(tvbs_g_state.bootstrap_secret);
        obs_data_set_string(settings, "bootstrap_secret", tvbs_g_state.bootstrap_secret);
    } else {
        sd->bootstrap_secret = bstrdup(existing_secret);
    }

    /* If the engine is up, forward these settings right away. */
    tvbs_push_settings_to_engine(sd);

    UNUSED_PARAMETER(source);
    return sd;
}

static void source_destroy(void *data)
{
    source_clear_cache(data);
    bfree(data);
}

static void source_update(void *data, obs_data_t *settings)
{
    struct tvbs_source_data *sd = data;
    source_clear_cache(sd);
    sd->target_input      = bstrdup(obs_data_get_string(settings, "target_input"));
    sd->ws_host           = bstrdup(obs_data_get_string(settings, "ws_host"));
    sd->ws_port           = (int)obs_data_get_int(settings, "ws_port");
    sd->ws_password       = bstrdup(obs_data_get_string(settings, "ws_password"));
    sd->ws_tls            = obs_data_get_bool(settings, "ws_tls");
    sd->scheduler_enabled = obs_data_get_bool(settings, "scheduler_enabled");
    sd->bootstrap_secret  = bstrdup(obs_data_get_string(settings, "bootstrap_secret"));

    tvbs_push_settings_to_engine(sd);
}

/* No video / audio frames — the source is "control plane only". */
static void source_render(void *data, gs_effect_t *effect)
{
    UNUSED_PARAMETER(data);
    UNUSED_PARAMETER(effect);
}

static struct obs_source_info source_info = {
    .id             = TVBS_SOURCE_ID,
    .type           = OBS_SOURCE_TYPE_INPUT,
    .output_flags   = OBS_SOURCE_VIDEO,   /* declared so OBS places it on the preview */
    .get_name       = source_get_name,
    .create         = source_create,
    .destroy        = source_destroy,
    .update         = source_update,
    .get_defaults   = source_defaults,
    .get_properties = source_get_properties,
    .video_render   = source_render,
};

/* --------------------------------------------------------------------------
 * Public
 * ------------------------------------------------------------------------ */

void tvbs_source_register(void)
{
    obs_register_source(&source_info);
}

void tvbs_source_unregister(void)
{
    /* libobs has no "unregister source" API; we rely on obs_module_unload
     * to clear us at process exit. The function exists for symmetry / future
     * reload support. */
}
