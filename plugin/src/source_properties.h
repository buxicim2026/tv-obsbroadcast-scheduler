/*
 * source_properties.h — Qt properties panel + bootstrap post for the
 * scheduler control source.
 */

#ifndef TVBS_SOURCE_PROPERTIES_H
#define TVBS_SOURCE_PROPERTIES_H

#include <obs-module.h>

struct tvbs_source_data;

/* Build the OBS properties panel. */
obs_properties_t *tvbs_source_build_properties(void *data);

/* Forward the current settings to the Rust engine via HTTP POST
 * /api/bootstrap. Runs synchronously (blocks the OBS UI for ~ms).
 * All callers should be on a non-UI thread; we don't queue here, but
 * the OBS worker thread on Windows still completes in well under
 * OBS's 5-second user-action timeout. */
void tvbs_push_settings_to_engine(struct tvbs_source_data *data);

#endif
