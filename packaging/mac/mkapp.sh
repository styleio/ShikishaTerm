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
#    MAC_SIGN_LOCAL=1   MAC_SIGN_IDENTITY is a certificate of this Mac's own,
#                       not Apple's (tools/debug/mkapp-test.mac.sh makes one):
#                       signed with it as it is, with no timestamp and no
#                       hardened runtime, for copies that never leave here. What
#                       the system was allowed for one build stays allowed for
#                       the next, as it does not for one signed ad hoc. Its
#                       Chromium keeps its key in a pretend keychain, which asks
#                       nothing when the next build comes
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
if [ "${MAC_RELEASE:-}" = 1 ] && [ "${MAC_SIGN_LOCAL:-}" = 1 ]; then
    echo "mkapp: a release is not signed with a certificate of this Mac's own" >&2
    exit 1
fi
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

# Chromium keeps the key its saved passwords and cookies are locked with in the
# keychain, under a name fixed in the framework: "Chromium Safe Storage", the
# same for every CEF app and every plain Chromium. Sharing it means one app's
# "Always Allow" opens the others' keys, and the keychain's question names
# whichever app asked first. CEF has no setting for it, so the one copy of the
# name is changed here, in the app's copy of the framework, to one of the same
# length (the bytes around it stay where they are). Exactly one is expected:
# none or more means CEF has changed how it keeps the name, and the build stops
# rather than ship the shared name, or a guess
KEYCHAIN_FROM="Chromium Safe Storage"
KEYCHAIN_TO="SHIKISHA Safe Storage"
CEF_BIN="$CONTENTS/Frameworks/$FRAMEWORK/Chromium Embedded Framework"
[ ${#KEYCHAIN_FROM} = ${#KEYCHAIN_TO} ] || { echo "mkapp: the keychain's names differ in length" >&2; exit 1; }
found=$(FROM=$KEYCHAIN_FROM TO=$KEYCHAIN_TO perl -0777 -pi -e '
    $n = s/\Q$ENV{FROM}\E\0/$ENV{TO}\0/g;
    END { print STDERR $n + 0 }
' "$CEF_BIN" 2>&1 >/dev/null)
[ "$found" = 1 ] || {
    echo "mkapp: \"$KEYCHAIN_FROM\" is in $FRAMEWORK $found times, not once: CEF keeps the keychain's name another way now, and packaging/mac/mkapp.sh has to learn it" >&2
    exit 1
}

# ── what the Mac's questions say ──────────────────────────────────────────
# Each reason the system shows when it asks the person for something, in the
# words of lang/*.json (the keys mac.ask.<the Info.plist key>), so that a Mac
# set to Japanese asks in Japanese. English is the app's own language and goes
# in Info.plist; every language, English too, goes in <lang>.lproj, where a
# Mac looks first. A sentence a language has not got is said in English
LANGS=""
for f in "$ROOT"/lang/*.json; do
    l=$(basename "$f" .json)
    plutil -convert xml1 -o "$WORK/lang-$l.plist" "$f"
    LANGS="$LANGS $l"
done
ASKS=$(sed -n 's/^  "mac\.ask\.\([A-Za-z]*\)".*/\1/p' "$ROOT/lang/en.json")
[ -n "$ASKS" ] || { echo "mkapp: lang/en.json has no mac.ask.* sentences, and a Mac asks nothing without them" >&2; exit 1; }
said() {
    /usr/libexec/PlistBuddy -c "Print :mac.ask.$2" "$WORK/lang-$1.plist" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c "Print :mac.ask.$2" "$WORK/lang-en.plist"
}
asks_plist() {
    # What the programs run in its terminals touch is asked for in this app's
    # name. Without a reason written here a Mac does not ask; it refuses, and
    # the program in the terminal just fails
    for k in $ASKS; do
        printf '    <key>%s</key><string>%s</string>\n' "$k" "$(said en "$k" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g')"
    done
}

# A copy signed with a certificate of this Mac's own has no team, and the
# keychain knows such an app by its binary: each new build would be asked for
# the password to Chromium's key. It uses the pretend keychain instead
# (src/browser/cef_engine.rs, extra_switches), and keeps its logins across builds
LOCAL_ENV=""
if [ "${MAC_SIGN_LOCAL:-}" = 1 ]; then
    LOCAL_ENV="<key>SHIKISHA_MOCK_KEYCHAIN</key><string>1</string>"
fi

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
    <key>CFBundleLocalizations</key><array>$(for l in $LANGS; do printf '<string>%s</string>' "$l"; done)</array>
    <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
    <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
    <!-- Chromium's own advice: the allocator's nano zone and Chromium's do not mix -->
    <key>LSEnvironment</key><dict><key>MallocNanoZone</key><string>0</string>$LOCAL_ENV</dict>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
    <key>NSRequiresAquaSystemAppearance</key><false/>
$(asks_plist)
</dict>
</plist>
PLIST
}

# Chromium starts its helpers by these names: the app's own, followed by what
# each one is for. A Mac's code signing wants one app per kind, each with
# what it is allowed to do (packaging/mac/helper.entitlements).
#
# Not "(Alerts)", the one Chromium shows a page's notifications from: that
# would be a second app in System Settings > Notifications, under another name
# than this program's. A page's notifications are this program's own
# (src/macnote.rs), and Chromium is refused them (src/browser/cef_engine.rs)
HELPERS="|.helper
 (GPU)|.helper.gpu
 (Renderer)|.helper.renderer
 (Plugin)|.helper.plugin"

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
plutil -lint -s "$CONTENTS/Info.plist"
for l in $LANGS; do
    mkdir -p "$CONTENTS/Resources/$l.lproj"
    {
        # The app's name, in every language the same, beside the reasons as
        # a Mac's own apps have it. Once, with reasons and no name here, the
        # system asked in the name of the bundle's file instead
        # ("SHIKISHA-TERM.app.bundle")
        printf '"CFBundleDisplayName" = "%s";\n"CFBundleName" = "%s";\n' "$NAME" "$NAME"
        for k in $ASKS; do
            printf '"%s" = "%s";\n' "$k" "$(said "$l" "$k" | sed 's/\\/\\\\/g; s/"/\\"/g')"
        done
    } > "$CONTENTS/Resources/$l.lproj/InfoPlist.strings"
    plutil -lint -s "$CONTENTS/Resources/$l.lproj/InfoPlist.strings"
done
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
if [ -n "${MAC_SIGN_IDENTITY:-}" ] && [ "${MAC_SIGN_LOCAL:-}" = 1 ]; then
    # The hardened runtime loads only libraries of the app's own team, and a
    # certificate of this Mac's own has no team: Chromium would not load
    sign() { codesign --force --sign "$MAC_SIGN_IDENTITY" "$@"; }
elif [ -n "${MAC_SIGN_IDENTITY:-}" ]; then
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
if [ -n "${MAC_SIGN_IDENTITY:-}" ] && [ "${MAC_SIGN_LOCAL:-}" != 1 ]; then
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
