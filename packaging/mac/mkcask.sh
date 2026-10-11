#!/bin/sh
#
#  Writes the Homebrew cask for a release, from the checksums of its two .dmg
#  files, so `brew install --cask styleio/shikisha-term/shikisha-term` installs
#  what the release page hands out.
#
#      packaging/mac/mkcask.sh <version> <folder holding the .dmg.sha256 files>
#
#  Printed. .github/workflows/homebrew.yml puts it in this project's own tap
#  when a release is published; CI reads it by Homebrew's rules on every push.
#  The cask says the app updates itself (auto_updates), so Homebrew does not
#  fight the app over its own updates.
#
set -eu

VERSION=${1:?usage: mkcask.sh <version> <folder>}
DIR=${2:?}
VERSION=${VERSION#v}

sum() {
    f="$DIR/SHIKISHA-TERM-mac-$1.dmg.sha256"
    [ -f "$f" ] || { echo "mkcask: no $f" >&2; exit 1; }
    cut -d' ' -f1 < "$f"
}
ARM=$(sum arm64)
INTEL=$(sum x64)

cat <<CASK
cask "shikisha-term" do
  arch arm: "arm64", intel: "x64"

  version "$VERSION"
  sha256 arm:   "$ARM",
         intel: "$INTEL"

  url "https://github.com/styleio/ShikishaTerm/releases/download/v#{version}/SHIKISHA-TERM-mac-#{arch}.dmg"
  name "SHIKISHA-TERM"
  desc "Terminal for running AI coding agents side by side, watched from anywhere"
  homepage "https://shikisha-term.com/"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :monterey

  app "SHIKISHA-TERM.app"

  zap trash: [
    "~/Library/Application Support/SHIKISHA-TERM",
    "~/Library/Caches/com.wiredxeco.shikisha-term",
    "~/Library/Caches/SHIKISHA-TERM",
    "~/Library/Preferences/com.wiredxeco.shikisha-term.plist",
    "~/Library/Saved Application State/com.wiredxeco.shikisha-term.savedState",
  ]
end
CASK
