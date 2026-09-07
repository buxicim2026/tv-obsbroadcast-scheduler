/*
 * platform/platform.c — anything that doesn't need OS-specific path.
 */

#include "platform.h"

/* Reserved for cross-platform helpers (sleep_ms, signal mask, etc.).
 * The init-scaffold stage leaves this empty; later todos will add what's
 * needed (e.g., a portable usleep wrapper). */
