#!/bin/sh
#
#  Puts SHIKISHA on a Linux machine, and refuses to put anything there that
#  this project did not sign.
#
#      curl -fsSL https://raw.githubusercontent.com/styleio/ShikishaTerm/main/packaging/linux/install.sh | sh
#
#  What it does, in order: work out which build this machine wants, download it
#  along with its checksum and its signature, check both, and only then unpack
#  it. Nothing is run, and nothing is put in place, before it has been checked.
#
#  The public key below is the one compiled into the program itself
#  (crates/core/src/update.rs), so an installed copy and a self-updating copy
#  trust exactly the same key. A test in the repository keeps the two equal —
#  edit one and the build says so.
#
#  Options:
#      --version vX.Y.Z   a particular release, rather than the newest
#      --prefix DIR       where the binary goes (default /usr/local/bin)
#      --service          enable and start the user service once installed
#      --no-service       do not write the service file at all
#
#  SHIKISHA_INSTALL_BASE points the download somewhere other than GitHub — a
#  mirror, or a folder served on a machine with no way out. The signature is
#  checked just the same, which is the point: where it came from decides
#  nothing, who signed it decides everything.
#
set -eu

REPO=styleio/ShikishaTerm
PUBLIC_KEY_PEM='-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEAk16isr1sZUfCE4TQGzWqV6nlAnNJoiDCFe73D+pHO7Q=
-----END PUBLIC KEY-----'

VERSION=latest
PREFIX=/usr/local/bin
SERVICE=ask

say() { printf '%s\n' "$*"; }
die() { printf 'shikisha: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version wants a tag";  VERSION=$2; shift 2 ;;
        --prefix)  [ $# -ge 2 ] || die "--prefix wants a folder"; PREFIX=$2;  shift 2 ;;
        --service)    SERVICE=yes ; shift ;;
        --no-service) SERVICE=no  ; shift ;;
        -h|--help) sed -n '3,27p' "$0" | sed 's/^#  \{0,1\}//'; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

# ── what this machine can do ──────────────────────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is needed and not here"; }
need tar
need openssl

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    read_url() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    read_url() { wget -qO - "$1"; }
else
    die "curl or wget is needed and neither is here"
fi

if command -v sha256sum >/dev/null 2>&1; then
    sum() { sha256sum "$1" | cut -d' ' -f1; }
else
    sum() { openssl dgst -sha256 "$1" | sed 's/.*= *//'; }
fi

# Ed25519 is one-shot: the whole file is the message, so it cannot be fed
# through a digest first. `-rawin` is how OpenSSL says that, and it arrived in
# OpenSSL 3.0. An older one cannot check this signature at all, and installing
# without checking is not on offer.
openssl pkeyutl -help 2>&1 | grep -q -- -rawin ||
    die "this OpenSSL cannot check an Ed25519 signature (3.0 or newer is needed; this is $(openssl version))"

[ "$(uname -s)" = Linux ] ||
    die "this installer is for Linux; on Windows take the zip from the Releases page"

case $(uname -m) in
    x86_64|amd64)  ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) die "no build for $(uname -m) yet" ;;
esac

# ── which release ─────────────────────────────────────────────────────────
if [ "$VERSION" = latest ]; then
    VERSION=$(read_url "https://api.github.com/repos/$REPO/releases/latest" |
              sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1)
    [ -n "$VERSION" ] || die "could not find out which release is the newest"
fi

NAME="shikisha-serve-$VERSION-$ARCH-linux.tar.gz"
BASE=${SHIKISHA_INSTALL_BASE:-"https://github.com/$REPO/releases/download/$VERSION"}

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

say "SHIKISHA $VERSION ($ARCH)"
say "  fetching $NAME"
fetch "$BASE/$NAME"        "$TMP/$NAME"        || die "could not download $NAME"
fetch "$BASE/$NAME.sha256" "$TMP/$NAME.sha256" || die "the release has no checksum"
fetch "$BASE/$NAME.sig"    "$TMP/$NAME.sig"    || die "the release is not signed, so it will not be installed"

# ── is it what it says it is ──────────────────────────────────────────────
want=$(cut -d' ' -f1 < "$TMP/$NAME.sha256")
got=$(sum "$TMP/$NAME")
[ "$want" = "$got" ] || die "the download does not match its checksum (wanted $want, got $got)"
say "  checksum ok"

# The signature is written as hex; OpenSSL wants the sixty-four bytes themselves
unhex() {
    h=$(tr -d '\n\r \t' < "$1")
    esc=''
    while [ -n "$h" ]; do
        byte=${h%"${h#??}"}
        h=${h#??}
        esc="$esc\\0$(printf '%03o' "$((0x$byte))")"
    done
    printf '%b' "$esc"
}

printf '%s\n' "$PUBLIC_KEY_PEM" > "$TMP/key.pem"
unhex "$TMP/$NAME.sig" > "$TMP/sig.bin"
openssl pkeyutl -verify -pubin -inkey "$TMP/key.pem" -rawin \
        -in "$TMP/$NAME" -sigfile "$TMP/sig.bin" >/dev/null 2>&1 ||
    die "the download is not signed with this project's key — nothing was installed"
say "  signature ok"

# ── put it where it goes ──────────────────────────────────────────────────
mkdir -p "$TMP/unpacked"
tar -xzf "$TMP/$NAME" -C "$TMP/unpacked"
BIN="$TMP/unpacked/shikisha-serve"
[ -f "$BIN" ] || die "the archive does not hold shikisha-serve"
chmod 755 "$BIN"

if [ -w "$PREFIX" ]; then
    install -m 755 "$BIN" "$PREFIX/shikisha-serve"
elif command -v sudo >/dev/null 2>&1 && [ -d "$PREFIX" ]; then
    say "  $PREFIX is not yours to write to; asking sudo"
    sudo install -m 755 "$BIN" "$PREFIX/shikisha-serve"
else
    PREFIX="$HOME/.local/bin"
    mkdir -p "$PREFIX"
    install -m 755 "$BIN" "$PREFIX/shikisha-serve"
    case ":$PATH:" in
        *":$PREFIX:"*) ;;
        *) say "  put $PREFIX on your PATH to run it by name" ;;
    esac
fi
say "  installed $PREFIX/shikisha-serve"

# ── the service ───────────────────────────────────────────────────────────
# Written from the copy inside the archive, which the signature covered. Not
# enabled unless asked: starting something that keeps running is the machine's
# owner's decision, not an installer's.
UNIT_SRC="$TMP/unpacked/shikisha.service"
if [ "$SERVICE" != no ] && [ -f "$UNIT_SRC" ] && command -v systemctl >/dev/null 2>&1; then
    UNIT="$HOME/.config/systemd/user/shikisha.service"
    mkdir -p "$(dirname "$UNIT")"
    sed "s|^ExecStart=.*|ExecStart=$PREFIX/shikisha-serve|" "$UNIT_SRC" > "$UNIT"
    systemctl --user daemon-reload 2>/dev/null || true
    say "  service file at $UNIT"
    if [ "$SERVICE" = yes ]; then
        loginctl enable-linger "$(id -un)" 2>/dev/null || true
        systemctl --user enable --now shikisha
        say "  service started"
    fi
fi

say ""
say "Next:"
say "  $PREFIX/shikisha-serve            run it here, and see the board's address"
if [ "$SERVICE" = ask ]; then
    say "  systemctl --user enable --now shikisha    keep it running"
    say "  loginctl enable-linger \"\$USER\"            and keep it running after you log out"
fi
