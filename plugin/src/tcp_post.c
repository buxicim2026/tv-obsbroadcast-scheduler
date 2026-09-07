/*
 * tcp_post.c — minimal blocking HTTP POST client used by the C plugin to
 * bootstrap the Rust engine. Suitable for one-shot in-thread calls; for
 * anything more serious use libcurl.
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#include <winsock2.h>
#include <ws2tcpip.h>
#pragma comment(lib, "ws2_32")
typedef SOCKET tvbs_sock_t_inner;
#define TVBS_INVALID_SOCK_INNER INVALID_SOCKET
#else
#include <unistd.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
typedef int tvbs_sock_t_inner;
#define TVBS_INVALID_SOCK_INNER (-1)
#endif

#include "tcp_post.h"

bool tvbs_tcp_init(void) {
#ifdef _WIN32
    WSADATA wsa;
    int rc = WSAStartup(MAKEWORD(2, 2), &wsa);
    return rc == 0;
#else
    return true;
#endif
}

void tvbs_tcp_shutdown(void) {
#ifdef _WIN32
    WSACleanup();
#endif
}

static int connect_loopback(const char *host, int port) {
    tvbs_sock_t_inner s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (s == TVBS_INVALID_SOCK_INNER) return -1;
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons((uint16_t)port);
    inet_pton(AF_INET, host, &addr.sin_addr);
    if (connect(s, (struct sockaddr *)&addr, sizeof(addr)) != 0) {
#ifdef _WIN32
        closesocket(s);
#else
        close(s);
#endif
        return -1;
    }
    return (int)s;
}

static int write_all(int fd, const char *buf, size_t len) {
    size_t off = 0;
    while (off < len) {
#ifdef _WIN32
        int n = send((SOCKET)fd, buf + off, (int)(len - off), 0);
#else
        ssize_t n = write(fd, buf + off, len - off);
#endif
        if (n <= 0) return -1;
        off += (size_t)n;
    }
    return 0;
}

static int read_some(int fd, char *buf, size_t cap) {
#ifdef _WIN32
    int n = recv((SOCKET)fd, buf, (int)cap, 0);
#else
    ssize_t n = read(fd, buf, cap);
#endif
    if (n <= 0) return -1;
    return n;
}

static void close_fd(int fd) {
#ifdef _WIN32
    closesocket((SOCKET)fd);
#else
    close(fd);
#endif
}

int tvbs_http_post_json(const char *host, int port, const char *path,
                        const char *body) {
    if (!host || !path || !body) return -1;
    int fd = connect_loopback(host, port);
    if (fd < 0) return -1;

    char header[2048];
    int header_len = snprintf(header, sizeof(header),
        "POST %s HTTP/1.1\r\n"
        "Host: %s:%d\r\n"
        "Content-Type: application/json\r\n"
        "Content-Length: %zu\r\n"
        "Connection: close\r\n"
        "\r\n",
        path, host, port, strlen(body));
    if (header_len <= 0 || (size_t)header_len >= sizeof(header)) {
        close_fd(fd);
        return -1;
    }
    if (write_all(fd, header, (size_t)header_len) < 0) {
        close_fd(fd);
        return -1;
    }
    if (write_all(fd, body, strlen(body)) < 0) {
        close_fd(fd);
        return -1;
    }

    /* Read response head — we just need the status code. */
    char buf[4096];
    int total = 0;
    while (total < (int)sizeof(buf) - 1) {
        int n = read_some(fd, buf + total, sizeof(buf) - 1 - total);
        if (n <= 0) break;
        total += n;
        /* Stop after the headers. */
        if (strstr(buf, "\r\n\r\n")) break;
    }
    buf[total] = '\0';
    close_fd(fd);

    if (strncmp(buf, "HTTP/1.1 ", 9) != 0 && strncmp(buf, "HTTP/1.0 ", 9) != 0) {
        return -1;
    }
    int status = 0;
    if (sscanf(buf, "%*s %d", &status) != 1) return -1;
    return status;
}
