#!/bin/sh
#
#  Makes SHIKISHA-TERM.app, and the .dmg and .zip it is handed out in, from
#  binaries that are already built.
#
#      packaging/mac/mkapp.sh <program> <helper> <cef> <version> <arch> [out]
#
#  <program> is the window's binary (target/<triple>/release/SHIKISHA-TERM),
#  <helper> the Chromium helper's (shikisha-chromium-helper), <cef> the CEF
#  distribution both were built against, as cef-dll-sys lays it out (the
#  folder holding Chromium Embedded Framework.framework and CREDITS.html; the
#  build puts it under CEF_PATH). <arch> is what the release
#  calls the machine: arm64 or x86_64. Run from the repository's root, where
#  dist.list names what ships with the program; the bridges for other machines
#  are expected in bridge/, as the release puts them there.
#
#  Signing is the environment's to say:
#
#    MAC_SIGN_IDENTITY  the Developer ID Application certificate's name, as the
#                       keychain knows it. Without it the app is signed for
#                       this machine only (ad hoc): it runs here, and a Mac
#                       that downloaded it refuses it
#    MAC_NOTARY_KEY, MAC_NOTARY_KEY_ID, MAC_NOTARY_ISSUER
#                       an App Store Connect API key (the .p8 file's path, its
#                       id, its issuer). With them the .dmg is notarized and
#                       the ticket stapled to it and to the app
#    MAC_RELEASE=1      a build to be handed out: it stops rather than go out
#                       unsigned or unnotarized. A Mac opens neither, and the
#                       person who downloaded it is told it is damaged
#
#  Run by the release workflow, and runnable by hand on a Mac: what is in the
#  app should be checkable without a tag and without CI.
#
set -eu

PROGRAM=${1:?usage: mkapp.sh <program> <helper> <cef> <version> <arch> [out]}
HELPER=${2:?}
CEF=${3:?}
VERSION=${4:?}
ARCH=${5:?}
OUT=${6:-.}

VERSION=${VERSION#v}
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

NAME=SHIKISHA-TERM
# The app's name to the system, for good: what it was granted (the folders,
# the camera) and what updates it are both tied to this. Changing it after a
# release makes every installed copy a stranger to the Mac it is on
ID=com.wiredxeco.shikisha-term
FRAMEWORK="Chromium Embedded Framework.framework"
# The oldest macOS the app opens on: CEF's own floor
MIN_MACOS=12.0

case "$ARCH" in
    arm64|x86_64) ;;
    *) echo "mkapp: no Mac is a $ARCH" >&2; exit 1 ;;
esac
[ -x "$PROGRAM" ] || { echo "mkapp: no program at $PROGRAM" >&2; exit 1; }
[ -x "$HELPER" ] || { echo "mkapp: no helper at $HELPER" >&2; exit 1; }
[ -d "$CEF/$FRAMEWORK" ] || { echo "mkapp: no $FRAMEWORK in $CEF" >&2; exit 1; }
if [ "${MAC_RELEASE:-}" = 1 ]; then
    for v in MAC_SIGN_IDENTITY MAC_NOTARY_KEY MAC_NOTARY_KEY_ID MAC_NOTARY_ISSUER; do
        eval "given=\${$v:-}"
        [ -n "$given" ] || { echo "mkapp: $v is not set, and a release a Mac refuses to open is no release" >&2; exit 1; }
    done
fi

APP="$WORK/$NAME.app"
CONTENTS="$APP/Contents"
mkdir -p "$CONTENTS/MacOS" "$CONTENTS/Frameworks" "$CONTENTS/Resources" "$OUT"

# ── the program and the Chromium it draws with ────────────────────────────
install -m 755 "$PROGRAM" "$CONTENTS/MacOS/$NAME"
# ditto keeps what cp -R loses: the framework's symbolic links (Versions/Current)
ditto "$CEF/$FRAMEWORK" "$CONTENTS/Frameworks/$FRAMEWORK"
# Chromium is built from thousands of others, and its credits are their
# licenses: they travel with it, as THIRD-PARTY-NOTICES.txt does for the rest
install -m 644 "$CEF/CREDITS.html" "$CONTENTS/Resources/CHROMIUM-CREDITS.html"

plist_main() {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleDisplayName</key><string>$NAME</string>
    <key>CFBundleExecutable</key><string>$NAME</string>
    <key>CFBundleIconFile</key><string>$NAME.icns</string>
    <key>CFBundleIdentifier</key><string>$ID</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>$NAME</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleLocalizations</key><array><string>en</string><string>ja</string></array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
    <!-- Chromium's own advice: the allocator's nano zone and Chromium's do not mix -->
    <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSRequiresAquaSystemAppearance</key><false/>
    <!-- What the programs run in its terminals touch is asked for in this
         app's name. Without a reason written here a Mac does not ask; it
         refuses, and the program in the terminal just fails -->
    <key>NSAppleEventsUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal wants to control another app.</string>
    <key>NSCameraUsageDescription</key><string>A program running in SHIKISHA-TERM, or a page open in it, wants to use the camera.</string>
    <key>NSMicrophoneUsageDescription</key><string>A program running in SHIKISHA-TERM, or a page open in it, wants to use the microphone.</string>
    <key>NSDesktopFolderUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal wants to read or change files on the Desktop.</string>
    <key>NSDocumentsFolderUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal wants to read or change files in Documents.</string>
    <key>NSDownloadsFolderUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal, or a page saving a file, wants to use the Downloads folder.</string>
    <key>NSRemovableVolumesUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal wants to use files on a removable drive.</string>
    <key>NSNetworkVolumesUsageDescription</key><string>A program running in a SHIKISHA-TERM terminal wants to use files on a network drive.</string>
    <key>NSLocalNetworkUsageDescription</key><string>SHIKISHA-TERM serves its screen to your phone and other computers on this network, and the programs in its terminals reach servers on it.</string>
    <key>NSAudioCaptureUsageDescription</key><string>SHIKISHA-TERM sends the sound of a page you are sharing to the phone or computer watching it, and nothing else this Mac plays.</string>
    <key>NSBluetoothAlwaysUsageDescription</key><string>A page open in SHIKISHA-TERM wants to use a Bluetooth device.</string>
    <key>NSWebBrowserPublicKeyCredentialUsageDescription</key><string>A page open in SHIKISHA-TERM wants to sign you in with a passkey.</string>
</dict>
</plist>
PLIST
}

# Chromium starts its helpers by these names: the app's own, followed by what
# each one is for. A Mac's code signing wants one app per kind, each with
# what it is allowed to do (packaging/mac/helper.entitlements)
HELPERS="|.helper
 (GPU)|.helper.gpu
 (Renderer)|.helper.renderer
 (Plugin)|.helper.plugin
 (Alerts)|.helper.alerts"

plist_helper() {
    cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDisplayName</key><string>$1</string>
    <key>CFBundleExecutable</key><string>$1</string>
    <key>CFBundleIdentifier</key><string>$ID$2</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>CFBundleName</key><string>$1</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string></dict>
    <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
    <key>LSUIElement</key><string>1</string>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST
}

plist_main > "$CONTENTS/Info.plist"
printf 'APPL????' > "$CONTENTS/PkgInfo"
echo "$HELPERS" | while IFS='|' read -r kind suffix; do
    helper="$NAME Helper$kind"
    h="$CONTENTS/Frameworks/$helper.app/Contents"
    mkdir -p "$h/MacOS"
    install -m 755 "$HELPER" "$h/MacOS/$helper"
    plist_helper "$helper" "$suffix" > "$h/Info.plist"
    printf 'APPL????' > "$h/PkgInfo"
done

# ── what ships with the program, read from Contents/Resources ─────────────
# dist.list alone decides. What sits beside the program elsewhere sits in
# Resources here (config::shipped_dir), folders kept; what is Windows' own --
# the ConPTY, the .cmd files -- does not come
section=""
while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
        ''|'#'*) continue ;;
        '['*']') section=$line; continue ;;
    esac
    case "$section" in
        '[beside-exe]'|'[package]') ;;
        *) continue ;;
    esac
    case "$line" in *.cmd) continue ;; esac
    found=0
    if [ "${line%/\*\*}" != "$line" ]; then
        dir=${line%/\*\*}
        if [ -d "$ROOT/$dir" ]; then
            mkdir -p "$CONTENTS/Resources/$dir"
            ditto "$ROOT/$dir" "$CONTENTS/Resources/$dir"
            found=1
        fi
    else
        # The pattern is a shell pattern on purpose: it is matched here the way
        # it reads
        for f in "$ROOT"/$line; do
            [ -f "$f" ] || continue
            rel=${f#"$ROOT"/}
            mkdir -p "$CONTENTS/Resources/$(dirname "$rel")"
            install -m 644 "$f" "$CONTENTS/Resources/$rel"
            found=1
        done
    fi
    [ "$found" = 1 ] || echo "mkapp: nothing matched $line" >&2
done < "$ROOT/dist.list"
# The bridges run on other machines: they keep the bit that lets them
chmod 755 "$CONTENTS/Resources"/bridge/shikisha-bridge-*-linux 2>/dev/null || true

# ── the icon ──────────────────────────────────────────────────────────────
ICONSET="$WORK/$NAME.iconset"
mkdir -p "$ICONSET"
SRC="$ROOT/assets/pwa/icon-512.png"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$SRC" --out "$ICONSET/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$SRC" --out "$ICONSET/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$ICONSET" -o "$CONTENTS/Resources/$NAME.icns"

# ── signing, from the inside out ──────────────────────────────────────────
# Each piece is signed before what holds it: a signature covers what is inside
# it, so signing the outside first would be undone by signing the inside
if [ -n "${MAC_SIGN_IDENTITY:-}" ]; then
    sign() { codesign --force --timestamp --options runtime --sign "$MAC_SIGN_IDENTITY" "$@"; }
else
    echo "mkapp: MAC_SIGN_IDENTITY is not set: signed for this Mac only" >&2
    sign() { codesign --force --sign - "$@"; }
fi
for lib in "$CONTENTS/Frameworks/$FRAMEWORK/Libraries"/*.dylib; do
    if [ -f "$lib" ]; then sign "$lib"; fi
done
sign "$CONTENTS/Frameworks/$FRAMEWORK"
echo "$HELPERS" | while IFS='|' read -r kind suffix; do
    sign --entitlements "$HERE/helper.entitlements" "$CONTENTS/Frameworks/$NAME Helper$kind.app"
done
sign --entitlements "$HERE/app.entitlements" "$APP"
codesign --verify --strict --deep "$APP"

# ── the .dmg and the .zip ─────────────────────────────────────────────────
# Named without the version: the copy already installed looks for exactly this
# name among a release's files to update itself (update.rs, zip_name), and a
# Mac's processors are called arm64 and x64 there, as the downloads page says
case "$ARCH" in
    arm64)  STEM="$NAME-mac-arm64" ;;
    x86_64) STEM="$NAME-mac-x64" ;;
esac
DMG_ROOT="$WORK/dmg"
mkdir -p "$DMG_ROOT"
ditto "$APP" "$DMG_ROOT/$NAME.app"
ln -s /Applications "$DMG_ROOT/Applications"
DMG="$OUT/$STEM.dmg"
rm -f "$DMG"
hdiutil create -quiet -volname "$NAME" -srcfolder "$DMG_ROOT" -fs HFS+ -format UDZO "$DMG"
if [ -n "${MAC_SIGN_IDENTITY:-}" ]; then
    codesign --force --timestamp --sign "$MAC_SIGN_IDENTITY" "$DMG"
fi

if [ -n "${MAC_NOTARY_KEY:-}" ]; then
    xcrun notarytool submit "$DMG" --key "$MAC_NOTARY_KEY" --key-id "$MAC_NOTARY_KEY_ID" \
        --issuer "$MAC_NOTARY_ISSUER" --wait
    # The ticket goes on both: the .dmg for the first opening, the app for a
    # Mac that later checks it with no network
    xcrun stapler staple "$DMG"
    xcrun stapler staple "$APP"
    spctl --assess --type execute --verbose "$APP"
else
    echo "mkapp: MAC_NOTARY_KEY is not set: not notarized" >&2
fi

ZIP="$OUT/$STEM.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
# The app itself too, to be opened where it was made
rm -rf "${OUT:?}/$NAME.app"
ditto "$APP" "$OUT/$NAME.app"
echo "$DMG"
echo "$ZIP"
echo "$OUT/$NAME.app"
