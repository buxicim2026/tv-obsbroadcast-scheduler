cmake_minimum_required(VERSION 3.16...3.28)

project(tv-obsbroadcast-scheduler VERSION 0.1.0 LANGUAGES C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)
if(NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release)
endif()

# ---------------------------------------------------------------------------
# OBS SDK. We only need the libobs *headers* plus something to satisfy the
# linker; real symbols resolve at runtime against the OBS installation.
#
#   -DLIBOBS_INCLUDE_DIR=<obs-studio source>/libobs
#
# Windows additionally wants an import library generated from the installed
# obs.dll (see scripts/package-plugin.ps1):
#   -DOBS_IMPORT_LIB=<path>/obs.lib
#
# Linux links a stub shared library with soname libobs.so.0:
#   -DOBS_STUB_LIB=<path>/libobs.so
#
# macOS uses -undefined dynamic_lookup and needs nothing extra.
# ---------------------------------------------------------------------------

if(NOT LIBOBS_INCLUDE_DIR)
  message(FATAL_ERROR
    "LIBOBS_INCLUDE_DIR is not set. Point it at an obs-studio checkout's "
    "libobs/ directory (headers only, no build required).")
endif()

# ---- Sources ---------------------------------------------------------------
set(PLUGIN_SOURCES
  src/plugin.c
  src/source.c
  src/source_properties.c
  src/engine_proc.c
  src/tcp_post.c
  src/platform/platform.c
  src/platform/windows.c
  src/platform/posix.c
  src/platform/bsd.c
)

add_library(tv-obsbroadcast-scheduler MODULE ${PLUGIN_SOURCES})
target_include_directories(tv-obsbroadcast-scheduler PRIVATE
  "${LIBOBS_INCLUDE_DIR}"
  "${CMAKE_CURRENT_SOURCE_DIR}/src"
)
set_target_properties(tv-obsbroadcast-scheduler PROPERTIES
  PREFIX ""
  OUTPUT_NAME "tv-obsbroadcast-scheduler"
)

# Windows: keep <windows.h> from pulling in <winsock.h> (OBS headers do
# include windows.h, and any winsock2.h that comes afterwards would then
# redefine sockaddr/fd_set/timeval etc.). All socket code uses winsock2.h.
if(WIN32)
  target_compile_definitions(tv-obsbroadcast-scheduler PRIVATE
    WIN32_LEAN_AND_MEAN
    NOMINMAX
  )
endif()

if(WIN32)
  target_link_libraries(tv-obsbroadcast-scheduler PRIVATE ws2_32)
  if(OBS_IMPORT_LIB)
    target_link_libraries(tv-obsbroadcast-scheduler PRIVATE "${OBS_IMPORT_LIB}")
  else()
    message(FATAL_ERROR
      "OBS_IMPORT_LIB is not set. Generate obs.lib from obs.dll with "
      "dumpbin+lib (see scripts/package-plugin.ps1).")
  endif()
elseif(APPLE)
  find_package(Threads REQUIRED)
  target_link_libraries(tv-obsbroadcast-scheduler PRIVATE Threads::Threads)
  target_link_options(tv-obsbroadcast-scheduler PRIVATE "-undefined" "dynamic_lookup")
  # MODULE library keeps the .so suffix on macOS too (OBS loads .so plugins);
  # do NOT set SUFFIX "" here — the packaging step copies *.so.
else()
  find_package(Threads REQUIRED)
  target_link_libraries(tv-obsbroadcast-scheduler PRIVATE Threads::Threads)
  if(OBS_STUB_LIB)
    target_link_libraries(tv-obsbroadcast-scheduler PRIVATE "${OBS_STUB_LIB}")
  else()
    message(FATAL_ERROR
      "OBS_STUB_LIB is not set. Create a stub libobs.so with soname "
      "libobs.so.0 (see scripts/package-plugin.sh).")
  endif()
endif()

if(MSVC)
  target_compile_options(tv-obsbroadcast-scheduler PRIVATE /W3)
else()
  target_compile_options(tv-obsbroadcast-scheduler PRIVATE
    -Wall -Wextra -Wno-unused-parameter
  )
endif()

# ---- Install (used by scripts/package-plugin.* to assemble the bundle) -----
install(TARGETS tv-obsbroadcast-scheduler
  RUNTIME DESTINATION .
  LIBRARY DESTINATION .
)

# Locale files travel with the bundle.
install(DIRECTORY data/locale DESTINATION ./data/locale)
