/*
 * tcp_post.h — tiny blocking HTTP POST helper used by the C plugin to push
 * settings into the Rust engine over loopback. Intentionally tiny: we only
 * need it for bootstrap (one call), so wrapping a real HTTP client is
 * overkill. Async behaviour is provided by the OBS plugin thread model —
 * tvbs_push_settings_to_engine simply spawns this on a worker thread.
 */

#ifndef TVBS_TCP_POST_H
#define TVBS_TCP_POST_H

#include <stdbool.h>
#include <stddef.h>

#ifdef _WIN32
#include <winsock2.h>
typedef SOCKET tvbs_sock_t;
#define TVBS_INVALID_SOCK INVALID_SOCKET
#else
#include <sys/types.h>
typedef int tvbs_sock_t;
#define TVBS_INVALID_SOCK (-1)
#endif

/* Initialize WinSock on Windows. Safe to call repeatedly (returns false on
 * the second call, which we treat as success). */
bool tvbs_tcp_init(void);
void tvbs_tcp_shutdown(void);

/* POST `body` to `host:port/path`. Returns HTTP status code on success, or
 * -1 on socket failure. The body is a JSON string and the Content-Type is
 * automatically set to application/json. */
int tvbs_http_post_json(const char *host, int port, const char *path,
                        const char *body);

#endif
