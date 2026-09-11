#!/bin/bash
#
#  Builds the apt and dnf repositories, signed, out of a folder of packages.
#
#      packaging/linux/mkrepo.sh <packages-dir> <out-dir>
#
#  <packages-dir> holds .deb and .rpm files -- this release's, and every older
#  one still being kept. Everything found is indexed, so an old version stays
#  installable by name and nothing silently disappears from under a pin.
#
#  Signing needs a keyring with the archive key in it. Point GNUPGHOME at one
#  and set KEY_FPR to the fingerprint; without them the indexes are built
#  unsigned, which is useful for checking the shape of the tree and useless for
#  anything else -- apt refuses an unsigned repository, and it is right to.
#
#  bash rather than sh: this one is only ever run by CI and by hand on a
#  developer's machine, never piped into an unknown shell on someone's server.
#
set -euo pipefail

IN=${1:?usage: mkrepo.sh <packages-dir> <out-dir>}
OUT=${2:?}
HERE=$(cd "$(dirname "$0")" && pwd)
KEY_FPR=${KEY_FPR:-}

ORIGIN="SHIKISHA-TERM"
LABEL="SHIKISHA-TERM"
SUITE="stable"
CODENAME="stable"
COMPONENT="main"

say() { printf '%s\n' "$*"; }

signing() { [ -n "$KEY_FPR" ]; }
if signing; then
    say "signing as $KEY_FPR"
else
    say "NO KEY: the indexes will be unsigned and no package manager will take them"
fi

rm -rf "$OUT"
mkdir -p "$OUT"
cp "$HERE/shikisha-archive-key.asc" "$OUT/shikisha-archive-key.asc"

# ── apt ───────────────────────────────────────────────────────────────────
# The pool is flat under one source name; apt does not care how it is laid out
# so long as Packages says where each file is, and one folder is easier to
# reason about than the alphabetical nesting Debian uses for tens of thousands
# of sources.
APT="$OUT/apt"
mkdir -p "$APT/pool/main/shikisha"
found_deb=$(find "$IN" -name '*.deb' | wc -l)
[ "$found_deb" -gt 0 ] || { echo "mkrepo: no .deb in $IN" >&2; exit 1; }
find "$IN" -name '*.deb' -exec cp {} "$APT/pool/main/shikisha/" \;

for arch in amd64 arm64; do
    dir="$APT/dists/$SUITE/$COMPONENT/binary-$arch"
    mkdir -p "$dir"
    # Paths in Packages are relative to the archive root, so the scan has to
    # run from there -- a Packages written from anywhere else names files apt
    # cannot find, and says nothing about it until someone tries to install
    ( cd "$APT" && apt-ftparchive --arch "$arch" packages pool > "dists/$SUITE/$COMPONENT/binary-$arch/Packages" )
    gzip -9nkf "$dir/Packages"
    cat > "$dir/Release" <<EOF
Archive: $SUITE
Component: $COMPONENT
Origin: $ORIGIN
Label: $LABEL
Architecture: $arch
EOF
done

( cd "$APT" && apt-ftparchive \
    -o "APT::FTPArchive::Release::Origin=$ORIGIN" \
    -o "APT::FTPArchive::Release::Label=$LABEL" \
    -o "APT::FTPArchive::Release::Suite=$SUITE" \
    -o "APT::FTPArchive::Release::Codename=$CODENAME" \
    -o "APT::FTPArchive::Release::Architectures=amd64 arm64" \
    -o "APT::FTPArchive::Release::Components=$COMPONENT" \
    -o "APT::FTPArchive::Release::Description=SHIKISHA-TERM, the runtime with no window" \
    release "dists/$SUITE" > "dists/$SUITE/Release" )

if signing; then
    # Both shapes. InRelease is what every apt since 2014 asks for first;
    # Release.gpg is what an older one falls back to, and costs one more file
    gpg --batch --yes --pinentry-mode loopback --local-user "$KEY_FPR" \
        --clearsign --output "$APT/dists/$SUITE/InRelease" "$APT/dists/$SUITE/Release"
    gpg --batch --yes --pinentry-mode loopback --local-user "$KEY_FPR" \
        --armor --detach-sign --output "$APT/dists/$SUITE/Release.gpg" "$APT/dists/$SUITE/Release"
fi

# ── dnf / yum ─────────────────────────────────────────────────────────────
# Two things are signed, and dnf checks them separately: every package, which
# is what `gpgcheck=1` reads, and the metadata, which is `repo_gpgcheck=1`.
# Signing only the metadata leaves a package that can be swapped in the pool.
RPM="$OUT/rpm"
mkdir -p "$RPM"
found_rpm=$(find "$IN" -name '*.rpm' | wc -l)
[ "$found_rpm" -gt 0 ] || { echo "mkrepo: no .rpm in $IN" >&2; exit 1; }
find "$IN" -name '*.rpm' -exec cp {} "$RPM/" \;

if signing; then
    # Told on the command line rather than through ~/.rpmmacros: this runs on a
    # machine that belongs to somebody, and a signing tool has no business
    # leaving a config file in their home folder. rpm's own default sign
    # command is right as it stands -- it only needs to be told which key
    for f in "$RPM"/*.rpm; do
        rpmsign --define "_gpg_name $KEY_FPR" --addsign "$f" >/dev/null
    done
fi

createrepo_c --quiet "$RPM"
if signing; then
    gpg --batch --yes --pinentry-mode loopback --local-user "$KEY_FPR" \
        --armor --detach-sign --output "$RPM/repodata/repomd.xml.asc" "$RPM/repodata/repomd.xml"
fi

# ── what a person types ───────────────────────────────────────────────────
# Served at the root, because somebody who opens the repository's address in a
# browser is somebody looking for exactly this.
BASE=${REPO_BASE_URL:-https://pkg.shikisha-term.com}
sed -e "s|@BASE@|$BASE|g" "$HERE/repo-index.html" > "$OUT/index.html"

say ""
say "apt: $(find "$APT/pool" -name '*.deb' | wc -l) package(s)"
say "dnf: $(find "$RPM" -maxdepth 1 -name '*.rpm' | wc -l) package(s)"
find "$OUT" -type f | sed "s|^$OUT|  .|" | sort
