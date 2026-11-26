#!/usr/bin/env bash
# Install synnamon on Ubuntu. Run with sudo from the repo root:
#   sudo packaging/install.sh [path/to/synnamon-binary]
# Idempotent. Does not touch PAM, sshd, or any auth configuration.
set -euo pipefail

BIN="${1:-target/release/synnamon}"
[ "$(id -u)" -eq 0 ] || { echo "run with sudo" >&2; exit 1; }
[ -x "$BIN" ] || { echo "binary not found: $BIN (build with: cargo build --release)" >&2; exit 1; }

id synnamon >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin synnamon
usermod -aG systemd-journal synnamon

install -d -o root -g synnamon -m 0750 /etc/synnamon
install -m 0755 "$BIN" /usr/local/bin/synnamon
install -m 0644 "$(dirname "$0")/synnamon.service" /etc/systemd/system/synnamon.service
systemctl daemon-reload

echo "Installed. Next:"
echo "  sudo synnamon config                      # guided setup"
echo "  sudo synnamon test                        # send a test alert"
echo "  sudo systemctl enable --now synnamon"
