#!/bin/bash
# build-flatpak-local.sh - build simpletaskmgr as a local flatpak bundle WITHOUT
# flatpak-builder (which fails on this machine/fdostack: no BuildPlatform on
# flathub, CDN blocks the flatpak client User-Agent on object fetches, and
# 1.16 prefers apps-only summary.idx making runtimes invisible).
#
# Strategy (validated end-to-end on this host, 2025-09):
#   * runtime/base:  org.gnome.Platform/50 (flathub, user scope)
#   * toolchain:     org.freedesktop.Sdk/25.08 + Sdk.Extension.rust-stable/25.08
#     extracted files, split so only GCC-critical files stay in gcc's -L dirs
#     (the fdo stack otherwise poisons ld's search with same-soname libs:
#      libharfbuzz, libgtk, libtinfo...)
#   * headers/.pc:   host (Debian trixie, GTK 4.18) via a read-only /app/hostroot
#     bind + PKG_CONFIG_SYSROOT_DIR; .pc files stripped of -L flags
#   * linking:       unversioned .so names bound to the *runtime* (gnome50)
#     libraries, so the final binary only NEEDs what ships in the app's
#     runtime (NEEDED: gtk4/pango/cairo/glib set + libgcc_s + libc only)
#   * packaging:     flatpak build-export + build-bundle + optional --user install
#
# Prereqs: flatpak >= 1.14, the four refs installed (user scope), host GTK dev
# headers (gtk-4.0), a C compiler for link-time stubs (any), cargo network.
#
# Usage:
#   ./build-flatpak-local.sh            # build + export + bundle -> ./simpletaskmgr.flatpak
#   ./build-flatpak-local.sh install    # ...and install into user scope + fix desktop wrapper
#
set -euo pipefail

APPID=org.simpletaskmgr.simpletaskmgr
SHORT=${APPID##*.}   # last component ("simpletaskmgr"): the name flatpak
                     # republishes the app icon under on the host, and the
                     # name the window/taskbar icon uses (WINDOW_ICON_NAME).
ROOT="$(cd "$(dirname "$0")" && pwd)"
WORK="${STM_WORK:-$HOME/.cache/stm-work}"   # same fs as the flatpak runtimes -> hardlink copy works
STMB="${STM_DIR:-/tmp/stmb}"
SDK_EXT=$HOME/.local/share/flatpak/runtime/org.freedesktop.Sdk.Extension.rust-stable/x86_64/25.08
SDK_BASE=$HOME/.local/share/flatpak/runtime/org.freedesktop.Sdk/x86_64/25.08
RUNTIME=$HOME/.local/share/flatpak/runtime/org.gnome.Platform/x86_64/50

die(){ echo "ERROR: $*" >&2; exit 1; }

filedir(){ # refs live at  <refdir>/<commit-hash>/files
  for d in "$1"/*/files; do [ -d "$d" ] && { echo "$d"; return 0; }; done
  return 1
}

echo "== [1/7] checking prereqs"
for r in "$SDK_EXT" "$SDK_BASE" "$RUNTIME"; do
  filedir "$r" >/dev/null 2>&1 || die "missing ref: $r (flatpak --user install flathub $(basename $(dirname $(dirname $r)))/x86_64/... )"
done
command -v pkgconf gcc pkg-config cargo >/dev/null || die "need host pkgconf, gcc, cargo"
pkg-config --exists gtk4 || die "need host gtk-4.0 dev headers (gtk4.pc)"

SDK_FILES=$(filedir "$SDK_BASE")
RUNTIME_LIB=$(filedir "$RUNTIME")/lib/x86_64-linux-gnu
echo "   sdk ext : $SDK_FILES"
echo "   runtime : $RUNTIME"

echo "== [2/7] preparing toolchain split (gcc-critical only in gcc -L dirs)"
rm -rf "$WORK"
mkdir -p "$WORK"
cp -al "$SDK_FILES" "$WORK/sdk"          # hardlinks: instant, no space cost
S="$WORK/sdk/lib/x86_64-linux-gnu"
# Keep: C runtime bits + binutils/gcc libs the compiler needs. Everything else
# (gtk/harfbuzz/pango/libtinfo/...) goes to appstack/ so gcc's hardwired -L
# search (which includes $S) never sees same-soname impostors.
cd "$S"
shopt -s nullglob
for f in *; do
  case "$f" in
    crt1.o|crti.o|crtn.o|Scrt1.o|libc.so|libc.so.6|libc_nonshared.a|libc.a|libgcc_s.so.1|libm.so.6|libquadmath.so.0|libas.*|libc_nonshared*|libbfd.*|libctf*|libsframe.*|libopcodes.*|libz.so.1|libzstd.so.1|liblzma.so.5|libisl.*|libdecnumber.*|libmpc.so.3|libgomp.so.1|libstdc++.so.6)
      : ;;
    *) mkdir -p ../appstack && mv -f "$f" ../appstack/ ;;
  esac
done
shopt -u nullglob
cd "$ROOT"
[ -f "$S/libc.so" ] && mv -f "$S/libc.so" "$S/libc.so.disabled" || true   # its /usr/lib/...-absolute GROUP members don't exist here
# the fdo SDK ships only versioned libm/libgcc_s; -lm/-lgcc_s need the unversioned name
ln -sf libm.so.6 "$S/libm.so"
ln -sf libgcc_s.so.1 "$S/libgcc_s.so"
mkdir -p "$WORK/toolslibs"
mkdir -p "$WORK/pcfix"
mkdir -p "$WORK/stub"
mkdir -p "$WORK/devlink"

echo "== [3/7] rust toolchain"
RUST_FILES=$(filedir "$SDK_EXT")
[ -x "$RUST_FILES/bin/cargo" ] || die "cargo not found in rust-stable extension at $RUST_FILES"

echo "== [4/7] pkg-config for the sandbox"
HOSTPKGCONF=$(command -v pkgconf)
HOSTPKGCONF_LIB=$(ldd "$HOSTPKGCONF" | awk '/libpkgconf/{print $3}' | head -1)
mkdir -p "$WORK/shim/bin" "$WORK/shim/lib"
cp "$HOSTPKGCONF" "$WORK/shim/bin/pkg-config"
cp -L "$HOSTPKGCONF_LIB" "$WORK/shim/lib/libpkgconf.so.3"
# host .pc files (GTK 4.18 era), with all -L flags stripped (they point at
# host dirs we do not want the linker chasing)
find /usr/lib/x86_64-linux-gnu/pkgconfig /usr/lib/pkgconfig /usr/share/pkgconfig -name '*.pc' 2>/dev/null | while read -r pc; do
  sed 's/-L[^ ]*//g' "$pc" > "$WORK/pcfix/$(basename "$pc")" || true
done
# stubs for .pc files the fdo stack has but the host dev install lacks
for stub in epoxy-1.0 vulkan graphene-1.0; do
  [ -f "$WORK/pcfix/$stub.pc" ] || {
    name=${stub%-*}; ver=${stub##*-}
    echo "Name: $name
Version: $ver
Libs: " > "$WORK/pcfix/$stub.pc"
  }
done

echo "== [5/7] link shims (runtime libs under unversioned names + glibc nonshared)"
for lib in gtk-4 gdk_pixbuf-2.0 pango-1.0 pangocairo-1.0 pangoft2-1.0 harfbuzz cairo cairo-gobject \
           glib-2.0 gobject-2.0 gio-2.0 gmodule-2.0 graphene-1.0 vulkan xkbcommon wayland-client \
           epoxy fribidi fontconfig; do
  real=$(ls "$RUNTIME_LIB"/lib${lib}.so.[0-9]* 2>/dev/null | head -1)
  if [ -z "$real" ]; then real=$(ls "$RUNTIME_LIB"/lib${lib}.so 2>/dev/null | head -1); fi
  if [ -n "$real" ]; then ln -sf "$(readlink -f "$real")" "$WORK/devlink/lib${lib}.so"; else echo "   WARN: no runtime lib for $lib"; fi
done
for n in librt.so libutil.so libdl.so libpthread.so; do
  gcc -shared -x c /dev/null -o "$WORK/stub/$n" || true
done
# glibc libc.so link script: point at the SDK's libc.so.6 + its nonshared archive
printf 'GROUP ( %s/lib/x86_64-linux-gnu/libc.so.6 AS_NEEDED ( /app/glibc/libc_nonshared.a ) )\n' "$WORK/sdk" > "$WORK/libc.so"
cp "$WORK/libc.so" "$WORK/devlink/libc.so"   # so rust's `-lc` also lands on glibc 2.42

echo "== [6/7] flatpak build (sandbox: gnome50 runtime + the above binds)"
B=" --bind-mount=/app/hostroot=/ --bind-mount=/app/opt/sdk=$WORK/sdk"
B="$B --bind-mount=/app/opt/rust=$RUST_FILES --bind-mount=/app/pcfix=$WORK/pcfix"
B="$B --bind-mount=/app/shim/bin/pkg-config=$HOSTPKGCONF --bind-mount=/app/shim/lib/libpkgconf.so.3=$HOSTPKGCONF_LIB"
B="$B --bind-mount=/app/toolslibs=$WORK/toolslibs --bind-mount=/app/glibc/libc_nonshared.a=$WORK/sdk/lib/x86_64-linux-gnu/libc_nonshared.a"
for n in librt.so libutil.so libdl.so libpthread.so; do B="$B --bind-mount=/app/devlink/$n=$WORK/stub/$n"; done
for l in "$WORK"/devlink/*.so; do B="$B --bind-mount=/app/devlink/$(basename "$l")=$(readlink -f "$l")"; done
B="$B --bind-mount=/usr/lib/x86_64-linux-gnu/libc.so=$WORK/libc.so"

rm -rf "$STMB"
flatpak build-init --arch=x86_64 "$STMB" "$APPID" org.gnome.Platform org.gnome.Platform 50

SB="$B --bind-mount=/src=$ROOT"
flatpak build $SB --share=network "$STMB" sh -c '
  set -e
  export PATH=/app/shim/bin:/app/opt/sdk/bin:/app/opt/rust/bin:$PATH
  export LD_LIBRARY_PATH=/app/shim/lib:/app/toolslibs:/app/opt/sdk/lib/x86_64-linux-gnu:/app/opt/sdk/lib
  export PKG_CONFIG=/app/shim/bin/pkg-config PKG_CONFIG_LIBDIR=/app/pcfix PKG_CONFIG_SYSROOT_DIR=/app/hostroot
  export CARGO_HOME=/app/cargo CARGO_TARGET_DIR=/app/target RUSTFLAGS="-C link-arg=-L/app/devlink -C link-arg=-L/app/opt/sdk/lib/x86_64-linux-gnu" CC=gcc
  mkdir -p /app/devlink
  cd /src && cargo build --release
'
mkdir -p "$STMB/files/bin"
cp "$STMB/files/target/release/simpletaskmgr" "$STMB/files/bin/simpletaskmgr"
# desktop + icons into the app layer
mkdir -p "$STMB/files/share/applications"
cp "$ROOT/share/org.simpletaskmgr.simpletaskmgr.desktop" "$STMB/files/share/applications/"
# Icon naming must line up in all three places or GNOME shows no dash icon:
#   * the bundle icon *files*: flatpak only keeps a file if its name is the
#     app id, so name them <appid>.png
#   * on install flatpak republishes the app icon on the host under the id's
#     SHORT name (simpletaskmgr.png) - that is the name that must resolve
#   * the desktop Icon= key is passed through unchanged, so set it to the
#     SHORT name (which is also WINDOW_ICON_NAME in src/ui.rs)
sed -i "s/^Icon=.*/Icon=$SHORT/" "$STMB/files/share/applications/$APPID.desktop"
for s in 48 64 128 256 512; do
  mkdir -p "$STMB/files/share/icons/hicolor/$s/apps"
  cp -f "$ROOT/icons/simpletaskmgr-$s.png" "$STMB/files/share/icons/hicolor/$s/apps/$APPID.png"
done

echo "== [7/7] export + bundle"
rm -rf /tmp/stm-repo
grep -q '^command=' "$STMB/metadata" || sed -i '/^\[Application\]/a command=simpletaskmgr' "$STMB/metadata"
flatpak build-finish "$STMB" --share=ipc --socket=wayland --socket=fallback-x11 --socket=pulseaudio --device=dri
flatpak build-export /tmp/stm-repo "$STMB"
flatpak build-bundle /tmp/stm-repo "$ROOT/simpletaskmgr.flatpak" "$APPID"
echo "BUNDLE: $ROOT/simpletaskmgr.flatpak ($(du -h "$ROOT/simpletaskmgr.flatpak"|awk '{print $1}'))"

if [ "${1:-}" = install ]; then
  flatpak --user uninstall -y "$APPID" 2>/dev/null || true
  flatpak --user install -y "$ROOT/simpletaskmgr.flatpak"
  W=$HOME/.local/share/applications/$APPID.desktop
  cp "$W" "$W.bak" 2>/dev/null || true
  cat > "$W" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=Simple Task Manager
GenericName=Process Manager
Comment=Watch processes on a live system (flatpak)
Exec=flatpak run $APPID
Terminal=false
Icon=$SHORT
Categories=System;Monitor;
StartupWMClass=simpletaskmgr
EOF
  echo "Installed. Launch with:  flatpak run $APPID   (or from the app grid; icon name: $SHORT)"
fi
