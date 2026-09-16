#!/usr/bin/env bash
# A throwaway SFTP server -- real OpenSSH -- for checking the file commands
# against the thing a person's server actually runs.
#
# The probe server in src/bin/sshd_probe.rs answers the same protocol, but it
# is this project's own code, so a client and a server written by the same
# hands can agree on a mistake. That is how sending a file that was not on the
# server yet came to fail against OpenSSH while passing against the probe.
#
# Nothing is installed. The packages are downloaded and unpacked under one
# folder in this user's home, and the server runs as this user, on the
# loopback only, on port 2222, accepting one key. Removing the folder removes
# all of it. Running this again stops the last one and starts from nothing.
#
# Run it inside WSL (Ubuntu, or any Debian-family Linux with apt):
#
#     bash tools/debug/sftp-server.wsl.sh
#
# then, on Windows, with what it printed:
#
#     SHIKISHA_LIVE_SFTP=127.0.0.1:2222
#     SHIKISHA_LIVE_SFTP_USER=<user>
#     SHIKISHA_LIVE_SFTP_KEY=<a copy of key, byte for byte>
#     SHIKISHA_LIVE_SFTP_ROOT=<serve>
#     cargo test -p shikisha-core live_sftp -- --ignored --test-threads=1
#
# Copy the key with `cp` onto /mnt/c/... rather than through a Windows text
# tool: a private key with CRLF line endings is refused as unreadable.
# Stop it with:  kill "$(cat ~/shikisha-sftp-check/etc/sshd.pid)"
set -euo pipefail

D="$HOME/shikisha-sftp-check"
PORT=2222

# Stop one left from a previous run before its pid file goes with the folder
if [ -f "$D/etc/sshd.pid" ]; then
  kill "$(cat "$D/etc/sshd.pid")" 2>/dev/null || true
  sleep 1
fi
rm -rf "$D"
mkdir -p "$D/pkg" "$D/root" "$D/etc" "$D/serve"

cd "$D/pkg"
apt download openssh-server openssh-sftp-server libwrap0 >/dev/null 2>&1
for f in ./*.deb; do dpkg -x "$f" "$D/root"; done

# The server's own key, and the one the client signs in with
ssh-keygen -q -t ed25519 -N "" -f "$D/etc/host_key"
ssh-keygen -q -t ed25519 -N "" -f "$D/etc/client_key"
cp "$D/etc/client_key.pub" "$D/etc/authorized_keys"
chmod 600 "$D/etc/authorized_keys" "$D/etc/host_key" "$D/etc/client_key"

# A server run by an ordinary user can only let that same user in, and only
# with a key -- which is all a check needs
cat > "$D/etc/sshd_config" <<EOF
Port $PORT
ListenAddress 127.0.0.1
HostKey $D/etc/host_key
PidFile $D/etc/sshd.pid
AuthorizedKeysFile $D/etc/authorized_keys
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
UsePAM no
StrictModes no
Subsystem sftp $D/root/usr/lib/openssh/sftp-server
EOF

# What the checks expect to find (see the live_sftp module in
# crates/core/src/hooks.rs): a site with a file to compare and a folder to
# walk, and a file outside it that no command told the site may reach
mkdir -p "$D/serve/site/assets/img"
printf 'one\ntwo\nthree\n' > "$D/serve/site/notes.txt"
printf 'hero-on-the-server' > "$D/serve/site/assets/img/hero.png"
mkdir -p "$D/serve/outside"
printf 'not yours' > "$D/serve/outside/secret.txt"

# Libraries the unpacked server needs and this machine does not have, found
# beside it rather than installed
export LD_LIBRARY_PATH="$D/root/usr/lib/x86_64-linux-gnu:$D/root/lib/x86_64-linux-gnu"
missing=$(ldd "$D/root/usr/sbin/sshd" | grep "not found" || true)
if [ -n "$missing" ]; then
  echo "still missing:"
  echo "$missing"
  exit 1
fi
"$D/root/usr/sbin/sshd" -f "$D/etc/sshd_config" -E "$D/etc/sshd.log"
sleep 1

echo "user=$(whoami)"
echo "serve=$D/serve"
echo "key=$D/etc/client_key"
if ss -ltn | grep -q "127.0.0.1:$PORT"; then
  echo "listening=yes"
else
  echo "listening=no"
  cat "$D/etc/sshd.log" 2>/dev/null || true
  exit 1
fi
