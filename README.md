# Synnamon

A small, fast system monitor that sends alerts to a Discord (or any HTTP) webhook. No UI, no database, one static-ish binary and no runtime beyond systemd.

**What it alerts on**

| Event | Alert |
|-------|-------|
| Battery drops below your threshold (default 20%) | once, then again only after it recovers and drops again |
| Successful SSH login | user, source IP, auth method |
| Successful local login (console, tty, desktop) | user, session |

Configuration is done through the tool itself (`synnamon config`), so there is no config file to go looking for.

**Supported:** Ubuntu 26.04 (and recent Debian/Ubuntu) on amd64, with systemd. Developed and tested against Ubuntu 26.04.

---

## Install

### Option 1: one-line installer (recommended)

```bash
curl -fsSL https://github.com/milo-v/synnamon/releases/latest/download/install.sh | sudo bash
```

This downloads the latest `.deb` from the GitHub release, verifies its SHA-256 checksum, and installs it with `apt`. To pin a version:

```bash
curl -fsSL https://github.com/milo-v/synnamon/releases/latest/download/install.sh | sudo SYNNAMON_VERSION=0.1.0 bash
```

Prefer to read the script first? Download it, inspect it, then run it:

```bash
curl -fsSLO https://github.com/milo-v/synnamon/releases/latest/download/install.sh
less install.sh
sudo bash install.sh
```

### Option 2: install the .deb by hand

```bash
curl -fsSLO https://github.com/milo-v/synnamon/releases/latest/download/synnamon_amd64.deb
curl -fsSLO https://github.com/milo-v/synnamon/releases/latest/download/SHA256SUMS
sha256sum -c SHA256SUMS
sudo apt install ./synnamon_amd64.deb
```

### Option 3: build from source

Requires Rust (stable) and a C toolchain (`sudo apt install build-essential`).

```bash
git clone https://github.com/milo-v/synnamon.git
cd synnamon
cargo install cargo-deb --locked
cargo build --release --locked
cargo deb --no-build
sudo apt install ./target/debian/synnamon_*_amd64.deb
```

The package installs the binary to `/usr/bin/synnamon`, a systemd unit, and creates an unprivileged `synnamon` user that is allowed to read the journal. It does **not** touch PAM, sshd, or any authentication configuration, The service is enabled and started automatically (it does nothing useful until you set a webhook below).

---

## Set up

1. **Create a Discord webhook:** in Discord, open the channel's settings → Integrations → Webhooks → New Webhook → Copy Webhook URL.
2. **Configure:**
   ```bash
   sudo synnamon config
   ```
   The guided setup asks for the webhook URL, the webhook type (`discord` or `generic`), whether to monitor the battery and at what percentage, and whether to alert on SSH and local logins. Press Enter to keep a value.
3. **Check it works:**
   ```bash
   sudo synnamon test
   ```
   A "Synnamon test" message should appear in your channel.

The service is already running after install, and it picks up config changes automatically within a couple of seconds, so there is nothing to restart or enable.

## Updating

```bash
synnamon update --check     # is a newer release available?
sudo synnamon update        # download, verify, and install it
```

`update` checks the latest GitHub release and, if it is newer than the installed version, runs that release's installer (which verifies the package checksum and installs it with `apt`). Your config is kept and the service is restarted. Running the one-line installer again does the same thing. Synnamon never updates itself in the background.

## Managing configuration

```bash
sudo synnamon config                         # guided setup
sudo synnamon config show                    # print all settings (webhook URL hidden)
sudo synnamon config get battery.threshold   # read one setting
sudo synnamon config set battery.threshold 30
```

| Key | Default | Meaning |
|-----|---------|---------|
| `webhook.url` | *(empty)* | Webhook to post to (secret; never printed) |
| `webhook.kind` | `discord` | `discord` or `generic` |
| `battery.enabled` | `true` | Monitor the battery |
| `battery.threshold` | `20` | Alert when below this percent (1-100) |
| `battery.poll_secs` | `60` | How often to check (minimum 5) |
| `logins.ssh` | `true` | Alert on successful SSH logins |
| `logins.local` | `true` | Alert on successful local logins |

Settings are stored in `/etc/synnamon/config.toml` (readable only by root and the `synnamon` service user, because it contains the webhook URL).

### Generic webhooks

With `webhook.kind` set to `generic`, Synnamon POSTs JSON to your URL:

```json
{"kind": "ssh_login", "title": "SSH login", "body": "milo logged in from 192.168.1.50 via publickey"}
```

`kind` is one of `battery`, `ssh_login`, `local_login`, `test`.

---

## Operating it

```bash
systemctl status synnamon        # is it running?
journalctl -u synnamon -f        # live logs
sudo systemctl restart synnamon
```

**Troubleshooting**

- *No alerts arrive:* run `sudo synnamon test`. If that fails, the URL is wrong or the network is blocked. Check `journalctl -u synnamon`.
- *No SSH/login alerts:* confirm the service user can read the journal: `id synnamon` should list `systemd-journal`. If you added the package before the group existed, run `sudo usermod -aG systemd-journal synnamon && sudo systemctl restart synnamon`.
- *"battery: no battery found":* normal on desktops and VMs; the battery monitor stays idle.
- *Cannot write config:* run config commands with `sudo`.

## Uninstall

```bash
sudo apt remove synnamon     # keep config
sudo apt purge synnamon      # also deletes /etc/synnamon (including the webhook URL) and the service user
```

## Security notes

- The webhook URL is a secret: whoever has it can post to your channel. It is stored in a root/`synnamon`-readable file and is never printed by `config show`, the wizard, or logs.
- Login alerts are read from the system journal, and only log lines written by root are trusted (so other local users cannot fake an alert with `logger`).
- Synnamon reports logins after they succeed. It is a notification tool, not an intrusion prevention system.

## Development

```bash
cargo test
cargo build --release
```

CI builds, tests, and packages inside an `ubuntu:26.04` container on every push (`.github/workflows/ci.yml`). To publish a release, bump `version` in `Cargo.toml`, then:

```bash
git tag v0.1.0 && git push origin v0.1.0
```

The release workflow builds the `.deb`, writes `SHA256SUMS`, and attaches both plus `install.sh` to a GitHub release. The tag must match the `Cargo.toml` version.

## License

[MIT](LICENSE)
