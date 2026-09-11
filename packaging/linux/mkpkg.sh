#!/bin/sh
#
#  Makes the .deb and the .rpm, from a binary that is already built.
#
#      packaging/linux/mkpkg.sh <binary> <version> <arch> [out]
#
#  <arch> is what the release calls it -- x86_64 or aarch64 -- and each format
#  is told the name it uses for the same machine. <version> may be a tag
#  (v0.9.0); the leading v is dropped, because neither format allows one.
#
#  Run by the release workflow, and runnable by hand: what is in a package
#  should be checkable without a tag and without CI.
#
set -eu

BIN=${1:?usage: mkpkg.sh <binary> <version> <arch> [out]}
VERSION=${2:?}
ARCH=${3:?}
OUT=${4:-.}

VERSION=${VERSION#v}
HERE=$(cd "$(dirname "$0")" && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

case "$ARCH" in
    x86_64)  DEB_ARCH=amd64 ;;
    aarch64) DEB_ARCH=arm64 ;;
    *) echo "mkpkg: no package name for $ARCH" >&2; exit 1 ;;
esac

# ── what goes in, laid out the way it will land ───────────────────────────
# The unit is a *user* unit and is not enabled by installing: it sits where
# every account on the machine can see it, and each person turns it on.
root="$WORK/root"
mkdir -p "$root/usr/bin" "$root/usr/lib/systemd/user" "$root/usr/lib/systemd/user-preset" \
         "$root/usr/share/man/man1" "$root/usr/share/doc/shikisha"
install -m 755 "$BIN" "$root/usr/bin/shikisha-serve"
install -m 644 "$HERE/shikisha.service" "$root/usr/lib/systemd/user/shikisha.service"
install -m 644 "$HERE/90-shikisha.preset" "$root/usr/lib/systemd/user-preset/90-shikisha.preset"
sed -i 's|^ExecStart=.*|ExecStart=/usr/bin/shikisha-serve|' "$root/usr/lib/systemd/user/shikisha.service"
gzip -9nc "$HERE/shikisha-serve.1" > "$root/usr/share/man/man1/shikisha-serve.1.gz"
chmod 644 "$root/usr/share/man/man1/shikisha-serve.1.gz"
install -m 644 "$HERE/../../LICENSE" "$root/usr/share/doc/shikisha/copyright"

DESCRIPTION="Run several AI coding agents side by side, and watch them from anywhere.
 SHIKISHA opens a terminal per agent, reads what each one is doing from what it
 prints, and serves that board over the network -- so a phone or a laptop can
 see and drive a machine nobody is sitting at. This package is the runtime with
 no window: one static binary and a systemd user service."

# ── the deb ───────────────────────────────────────────────────────────────
if command -v dpkg-deb >/dev/null 2>&1; then
    mkdir -p "$root/DEBIAN"
    SIZE=$(du -ks "$root" | cut -f1)
    cat > "$root/DEBIAN/control" <<CONTROL
Package: shikisha
Version: $VERSION
Section: devel
Priority: optional
Architecture: $DEB_ARCH
Maintainer: styleio <naofumi@te0.jp>
Installed-Size: $SIZE
Homepage: https://github.com/styleio/ShikishaTerm
Description: $DESCRIPTION
CONTROL
    DEB="$OUT/shikisha_${VERSION}_${DEB_ARCH}.deb"
    dpkg-deb --build --root-owner-group "$root" "$DEB" >/dev/null
    echo "$DEB"
    rm -rf "$root/DEBIAN"
else
    echo "mkpkg: dpkg-deb is not here; no .deb made" >&2
fi

# ── the rpm ───────────────────────────────────────────────────────────────
if command -v rpmbuild >/dev/null 2>&1; then
    top="$WORK/rpm"
    mkdir -p "$top/BUILD" "$top/RPMS" "$top/SPECS"
    # rpmbuild wants the files under a build root it is told about, so the
    # same tree is handed over rather than laid out twice
    cat > "$top/SPECS/shikisha.spec" <<SPEC
Name:           shikisha
Version:        $VERSION
Release:        1
Summary:        Run several AI coding agents side by side, and watch them from anywhere
License:        MIT
URL:            https://github.com/styleio/ShikishaTerm
BuildArch:      $ARCH
# One static binary: it needs nothing, and saying otherwise would make the
# package refuse to install on a machine where it would have run
AutoReqProv:    no

%description
SHIKISHA opens a terminal per agent, reads what each one is doing from what it
prints, and serves that board over the network -- so a phone or a laptop can
see and drive a machine nobody is sitting at. This package is the runtime with
no window: one static binary and a systemd user service.

%install
cp -a $root/. %{buildroot}/

%files
/usr/bin/shikisha-serve
/usr/lib/systemd/user/shikisha.service
/usr/lib/systemd/user-preset/90-shikisha.preset
/usr/share/man/man1/shikisha-serve.1.gz
%license /usr/share/doc/shikisha/copyright

%changelog
SPEC
    rpmbuild --define "_topdir $top" --define "_build_id_links none" \
             -bb "$top/SPECS/shikisha.spec" >/dev/null
    RPM=$(find "$top/RPMS" -name '*.rpm' | head -1)
    cp "$RPM" "$OUT/"
    echo "$OUT/$(basename "$RPM")"
else
    echo "mkpkg: rpmbuild is not here; no .rpm made" >&2
fi
