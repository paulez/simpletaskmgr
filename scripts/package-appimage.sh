#!/usr/bin/env bash
#
# package-appimage.sh — build a self-contained, portable Simple Task Manager
# AppImage (single file, double-click to run) from an already-built release
# binary, bundling the GTK 4 shared-library closure so the AppImage has no
# system dependencies beyond the base C library.
#
# Why AppImage (and not Flatpak/Snap): a task manager must see every host
# process. Flatpak/Snap put the app in a private PID namespace and disable
# ptrace, so they can only ever list their own sandbox. An AppImage does not
# unshare the PID namespace, so the app runs in the host PID namespace and
# enumerates the full process list. (Verified: an AppImage build of this app
# renders under a headless compositor and lists host PIDs.)
#
# Bundle strategy: we ship the GTK 4 / glib / cairo / freetype / X & Wayland
# client libs (everything that varies between distributions) and deliberately
# leave the glibc core (libc, libm, ld-linux), the C++ runtime (libstdc++) and
# libgcc_s to the host. That matches our "modern desktop / GTK 4.18 floor"
# target and keeps the image ~14 MB.
#
# Requirements (installed by CI; for local runs put them on PATH or set the
# env below): `mksquashfs` and `unsquashfs` (squashfs-tools), `curl`, `ldd`,
# and a release binary of `simpletaskmgr`. No FUSE is required at build time —
# appimagetool is extracted and run as a plain binary.
#
# Outputs:  <OUT>  (default: dist/simpletaskmgr-<version>-x86_64.AppImage)
#           and its SHA256 on stdout.
#
# Environment overrides:
#   STMB_VERSION          version string (default: read from Cargo.toml)
#   STMB_BIN              path to the release binary
#   STMB_OUT              output .AppImage path
#   STMB_APPDIR           scratch AppDir location (default: mktemp)
#   STMB_SQUASHFS_BIN_DIR directory holding mksquashfs/unsquashfs
#   STMB_TOOL_CACHE       where to cache the pinned appimagetool (default: ~/.cache/stm-appimage)
#   STMB_APPIMAGETOOL_URL pinned appimagetool download (default: appimagetool 1.9.1)

set -euo pipefail

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# --- repository root (this lives in scripts/) -------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# --- inputs -----------------------------------------------------------------
VERSION="${STMB_VERSION:-$(grep -m1 '^version[[:space:]]*=' "${REPO_ROOT}/Cargo.toml" | cut -d'"' -f2)}"
BIN="${STMB_BIN:-${REPO_ROOT}/target/release/simpletaskmgr}"
OUT="${STMB_OUT:-${REPO_ROOT}/dist/simpletaskmgr-${VERSION}-x86_64.AppImage}"
ICONS_DIR="${REPO_ROOT}/icons"

[ -x "${BIN}" ]       || die "release binary not found or not executable: ${BIN} (run 'cargo build --release' first)"
[ -d "${ICONS_DIR}" ] || die "icons dir not found: ${ICONS_DIR}"

# --- squashfs-tools ---------------------------------------------------------
if ! command -v mksquashfs >/dev/null 2>&1 || ! command -v unsquashfs >/dev/null 2>&1; then
  if [ -n "${STMB_SQUASHFS_BIN_DIR:-}" ] && [ -x "${STMB_SQUASHFS_BIN_DIR}/mksquashfs" ]; then
    export PATH="${STMB_SQUASHFS_BIN_DIR}:${PATH}"
  fi
fi
command -v mksquashfs >/dev/null 2>&1  || die "mksquashfs not found — install squashfs-tools (or set STMB_SQUASHFS_BIN_DIR)"
command -v unsquashfs >/dev/null 2>&1  || die "unsquashfs not found — install squashfs-tools (or set STMB_SQUASHFS_BIN_DIR)"

# --- pinned appimagetool (extracted so the build never needs FUSE) ----------
AIT_URL="${STMB_APPIMAGETOOL_URL:-https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-x86_64.AppImage}"
# Key the cache by a hash of the URL so a different pinned version can never be
# silently served from a stale cached build (keeps the pin reproducible).
AIT_HASH="$(printf '%s' "${AIT_URL}" | sha256sum | cut -c1-16)"
CACHE="${STMB_TOOL_CACHE:-${HOME}/.cache/stm-appimage}/appimagetool-${AIT_HASH}"
AIT_RUN="${CACHE}/squashfs-root/AppRun"
if [ ! -x "${AIT_RUN}" ]; then
  mkdir -p "${CACHE}"
  log "Downloading appimagetool (pinned) -> ${CACHE}/at.AppImage"
  curl -fsSL --retry 3 -o "${CACHE}/at.AppImage" "${AIT_URL}"
  chmod +x "${CACHE}/at.AppImage"
  log "Extracting appimagetool (FUSE-free build path)"
  ( cd "${CACHE}" && ./at.AppImage --appimage-extract >/dev/null )
fi
[ -x "${AIT_RUN}" ] || die "appimagetool did not extract to ${AIT_RUN}"
log "Using appimagetool: $( "${AIT_RUN}" --version 2>/dev/null | head -n1 )"

# --- transitive shared-library closure (worklist BFS over `ldd`) ------------
log "Computing the shared-library closure of $(basename "${BIN}")"
closure_of() {
  local root="$1"
  local -A have=()
  local -a queue=()
  local entry
  # Seed with the binary's reported dependencies.
  while IFS= read -r entry; do
    [ -n "${entry}" ] || continue
    if [ -z "${have[${entry}]:-}" ]; then
      have["${entry}"]=1
      queue+=("${entry}")
    fi
  done < <( ldd "${root}" 2>/dev/null | awk '/=> /{print $3}' | grep -E '^/' )
  # Expand each library's own dependencies until the set is closed.
  local i=0 f dep
  while [ "${i}" -lt "${#queue[@]}" ]; do
    f="${queue[${i}]}"
    while IFS= read -r dep; do
      [ -n "${dep}" ] || continue
      if [ -z "${have[${dep}]:-}" ]; then
        have["${dep}"]=1
        queue+=("${dep}")
      fi
    done < <( ldd "${f}" 2>/dev/null | awk '/=> /{print $3}' | grep -E '^/' )
    i=$((i + 1))
  done
  printf '%s\n' "${queue[@]}" | sort -u
}
mapfile -t LIBS < <( closure_of "${BIN}" )

# --- exclude the host-supplied core (glibc family + C++ runtime) ------------
is_host_lib() {
  case "$(basename "$1")" in
    libc.so.6|libm.so.6|ld-linux-x86-64.so.2|\
    libpthread*|librt.so.*|libdl.so.*|libutil*|libresolv*|\
    libnss_*|libcid*|libanl*|libstdc++.so.*|libgcc_s.so.*) return 0 ;;
    *) return 1 ;;
  esac
}

# --- assemble the AppDir ----------------------------------------------------
APPDIR="${STMB_APPDIR:-$(mktemp -d "${TMPDIR:-/tmp}/stmb-appdir-XXXXXX")}"
mkdir -p \
  "${APPDIR}/usr/bin" \
  "${APPDIR}/usr/lib/x86_64-linux-gnu" \
  "${APPDIR}/usr/share/applications" \
  "${APPDIR}/usr/share/icons/hicolor"
BUNDLE_LIB_DIR="${APPDIR}/usr/lib/x86_64-linux-gnu"
trap 'rm -rf "${APPDIR}"' EXIT

cp "${BIN}" "${APPDIR}/usr/bin/simpletaskmgr"
chmod +x "${APPDIR}/usr/bin/simpletaskmgr"

# icons (all sizes, for the app grid + embedded window icon)
for pair in 48x48:simpletaskmgr-48 64x64:simpletaskmgr-64 128x128:simpletaskmgr-128 \
            256x256:simpletaskmgr-256 512x512:simpletaskmgr-512; do
  dir="${pair%%:*}"; base="${pair##*:}"
  mkdir -p "${APPDIR}/usr/share/icons/hicolor/${dir}/apps"
  [ -f "${ICONS_DIR}/${base}.png" ] || die "missing icon ${ICONS_DIR}/${base}.png"
  cp "${ICONS_DIR}/${base}.png" "${APPDIR}/usr/share/icons/hicolor/${dir}/apps/simpletaskmgr.png"
done
cp "${ICONS_DIR}/simpletaskmgr-256.png" "${APPDIR}/simpletaskmgr.png"

# desktop entry
cat > "${APPDIR}/usr/share/applications/simpletaskmgr.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=Simple Task Manager
Comment=CPU/Mem/Disk graphs and a process list with Terminate/Kill
Exec=simpletaskmgr
Icon=simpletaskmgr
Terminal=false
Categories=System;Monitor;
EOF
cp "${APPDIR}/usr/share/applications/simpletaskmgr.desktop" "${APPDIR}/simpletaskmgr.desktop"

# AppRun: prepend the bundled libs on the loader path, then exec the app.
cat > "${APPDIR}/AppRun" <<'RUN'
#!/bin/sh
DIR="$(cd "$(dirname "$0")" && pwd)"
export LD_LIBRARY_PATH="${DIR}/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
exec "${DIR}/usr/bin/simpletaskmgr" "$@"
RUN
chmod +x "${APPDIR}/AppRun"

# --- bundle the closure (minus the host core) --------------------------------
bundled=0 skipped=0
for l in "${LIBS[@]}"; do
  base="$(basename "${l}")"
  if is_host_lib "${l}"; then
    skipped=$((skipped + 1)); continue
  fi
  if cp -L "${l}" "${BUNDLE_LIB_DIR}/${base}"; then
    bundled=$((bundled + 1))
  else
    log "warn: could not bundle ${l}"
  fi
done
log "Bundled ${bundled} libraries, left ${skipped} to the host (glibc / C++ runtime)."

# --- sanity: every NEEDED object must resolve (bundled or host) --------------
log "Verifying the bundled app resolves all shared objects"
missing="$( LD_LIBRARY_PATH="${BUNDLE_LIB_DIR}" ldd "${APPDIR}/usr/bin/simpletaskmgr" 2>/dev/null | awk '/not found/{print $1}' )" || true
if [ -n "${missing}" ]; then
  die "shared objects unresolved after bundling: ${missing}"
fi

# --- build the AppImage ------------------------------------------------------
mkdir -p "$(dirname "${OUT}")"
log "Building ${OUT}"
( cd "${CACHE}" && ARCH=x86_64 "${AIT_RUN}" "${APPDIR}" "${OUT}" ) 2>&1 | tail -n 6 || die "appimagetool failed"
[ -f "${OUT}" ] || die "AppImage was not produced at ${OUT}"

# --- report -----------------------------------------------------------------
size="$(du -h "${OUT}" 2>/dev/null | cut -f1)"
log "Done: ${OUT} (${size})"
sha256sum "${OUT}"
