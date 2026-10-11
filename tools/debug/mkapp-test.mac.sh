#!/bin/sh
#
#  Makes SHIKISHA-TERM.app for checking on this Mac, signed with a certificate
#  of this Mac's own instead of ad hoc.
#
#      sh tools/debug/mkapp-test.mac.sh <program> <helper> <cef> <out>
#
#  The arguments are packaging/mac/mkapp.sh's, without the version and the
#  machine (v0.0.0, this Mac's). For example, from the repository's root:
#
#      CEF=$(find ~/cef -maxdepth 2 -type d -name 'cef_macos_*' | head -1)
#      sh tools/debug/mkapp-test.mac.sh target/debug/SHIKISHA-TERM \
#          target/debug/shikisha-chromium-helper "$CEF" target/mac
#
#  Why. A Mac remembers what an app was allowed -- recording the screen, the
#  sound other apps play, a folder -- by the app's signature. Signed ad hoc,
#  every build is a stranger, and the person at the Mac is asked all of it
#  again. Signed with the same certificate every time, what was allowed once
#  stays allowed.
#
#  The certificate is made the first time, kept in a keychain file of its own
#  (SHIKISHA_TEST_SIGNING, or else ~/.shikisha-test-signing) and never in the
#  login keychain. That keychain's password is written below: it holds this
#  one certificate, which nobody but this Mac trusts and which signs nothing
#  that leaves it. The keychain is put in the list codesign looks in only while
#  signing, and the list is put back as it was afterwards. Nothing here asks the
#  person anything.
#
#  CI and the release do not use this: they sign ad hoc and with Developer ID
#  (packaging/mac/mkapp.sh).
set -eu

[ $# -eq 4 ] || { echo "usage: mkapp-test.mac.sh <program> <helper> <cef> <out>" >&2; exit 2; }
HERE=$(cd "$(dirname "$0")" && pwd)
ROOT=$(cd "$HERE/../.." && pwd)
DIR=${SHIKISHA_TEST_SIGNING:-$HOME/.shikisha-test-signing}
KEYCHAIN="$DIR/test-copy.keychain-db"
PASSWORD=shikisha-test-copy
NAME="SHIKISHA-TERM Test Copy"

case "$(uname -m)" in
    arm64) ARCH=arm64 ;;
    *) ARCH=x86_64 ;;
esac

if [ ! -f "$KEYCHAIN" ]; then
    mkdir -p "$DIR"
    chmod 700 "$DIR"
    WORK=$(mktemp -d)
    trap 'rm -rf "$WORK"' EXIT INT TERM
    cat > "$WORK/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
    # /usr/bin/openssl: the one every Mac has, and whose .p12 the keychain reads
    /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$WORK/cert.cnf" \
        -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null
    /usr/bin/openssl pkcs12 -export -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
        -name "$NAME" -passout pass:"$PASSWORD" -out "$WORK/id.p12"
    security create-keychain -p "$PASSWORD" "$KEYCHAIN"
    # Not locked again after a while: it is unlocked by this script each time
    security set-keychain-settings "$KEYCHAIN"
    security unlock-keychain -p "$PASSWORD" "$KEYCHAIN"
    security import "$WORK/id.p12" -k "$KEYCHAIN" -P "$PASSWORD" -T /usr/bin/codesign >/dev/null
    # codesign may use the key without asking
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$PASSWORD" "$KEYCHAIN" >/dev/null
    echo "mkapp-test: made the certificate \"$NAME\" in $KEYCHAIN" >&2
fi

security unlock-keychain -p "$PASSWORD" "$KEYCHAIN"
# By its hash: nobody trusts it, so codesign does not find it by its name
IDENTITY=$(security find-identity -p codesigning "$KEYCHAIN" | sed -n 's/^ *1) \([0-9A-F]\{40\}\) .*/\1/p' | head -1)
[ -n "$IDENTITY" ] || { echo "mkapp-test: no certificate in $KEYCHAIN" >&2; exit 1; }

# The list of keychains, as it was, and this one added only while signing
LIST=$(security list-keychains -d user | sed 's/^ *"//; s/" *$//')
restore() {
    # One per line, as `security list-keychains` printed them
    echo "$LIST" | tr '\n' '\0' | xargs -0 security list-keychains -d user -s
}
trap 'restore' EXIT INT TERM
echo "$LIST" | tr '\n' '\0' | xargs -0 sh -c 'security list-keychains -d user -s "$@" "'"$KEYCHAIN"'"' sh

MAC_SIGN_IDENTITY=$IDENTITY MAC_SIGN_LOCAL=1 \
    sh "$ROOT/packaging/mac/mkapp.sh" "$1" "$2" "$3" v0.0.0 "$ARCH" "$4"
