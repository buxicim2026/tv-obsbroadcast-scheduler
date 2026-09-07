/*
 * dock.c — registers the "Broadcast Scheduler" custom dock.
 *
 * libobs offers `obs_register_dock_id()` since OBS 25; it lets us declare a
 * dock slot that the user activates via:
 *     View → Docks → Custom Browser Docks → Add
 * and points at the URL we return. We point at the locally-running Rust
 * engine's /admin endpoint; that endpoint serves the in-OBS admin UI.
 */

#include <obs-module.h>
#include <util/platform.h>

#include <stdio.h>
#include <string.h>

#include "plugin.h"

static const char *dock_get_name(const char *id)
{
    UNUSED_PARAMETER(id);
    return obs_module_text("SchedulerDock");
}

static const char *dock_get_url(const char *id)
{
    UNUSED_PARAMETER(id);
    /* Lazy: the engine may not be up yet; the browser dock will retry. */
    static char url[64];
    snprintf(url, sizeof(url), "http://127.0.0.1:%d/admin",
             TVBS_DEFAULT_BOOTSTRAP_PORT);
    return url;
}

static struct obs_dock_info dock_info = {
    .get_name = dock_get_name,
    .get_url  = dock_get_url,
};

#define TVBS_DOCK_ID "tvbs_dock"

void tvbs_dock_register(void)
{
    obs_register_dock_id(TVBS_DOCK_ID, dock_info);
}

void tvbs_dock_unregister(void)
{
    /* No unregister API for docks; surviving until obs_module_unload. */
}
