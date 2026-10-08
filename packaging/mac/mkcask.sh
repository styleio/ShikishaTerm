#!/bin/sh
#
#  Writes the Homebrew cask for a release, from the checksums of its two .dmg
#  files, so `brew install --cask shikisha-term` installs what the release page
#  hands out.
#
#      packaging/mac/mkcask.sh <version> <folder holding the .dmg.sha256 files>
#
#  Printed, not published: where the cask is kept (a tap of this project's own,
#  or Homebrew's) is a person's decision. The cask says the app updates itself
#  (auto_updates), so Homebrew does not fight the app over its own updates.
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
  desc "Terminal for running AI coding agents side by side and watching them from anywhere"
  homepage "https://github.com/styleio/ShikishaTerm"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: ">= :monterey"

  app "SHIKISHA-TERM.app"

  zap trash: [
    "~/Library/Application Support/SHIKISHA-TERM",
    "~/Library/Application Support/ShikishaTerm",
    "~/Library/Caches/com.wiredxeco.shikisha-term",
    "~/Library/Preferences/com.wiredxeco.shikisha-term.plist",
    "~/Library/Saved Application State/com.wiredxeco.shikisha-term.savedState",
  ]
end
CASK
