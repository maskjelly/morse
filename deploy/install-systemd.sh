#!/usr/bin/env bash
set -euo pipefail
if [[ $(uname -s) != Linux || $EUID != 0 ]]; then
  echo "Run as root on a Linux VPS or inside your Linux microVM: sudo bash deploy/install-systemd.sh target/release/morse" >&2
  exit 1
fi
binary=${1:?Pass the path to a built Linux morse binary}
[[ -x "$binary" ]] || { echo "Binary is missing or not executable: $binary" >&2; exit 1; }
"$binary" --version
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
if ! id morse >/dev/null 2>&1; then useradd --create-home --shell /bin/bash morse; fi
install -d -o morse -g morse -m 700 /home/morse/.morse /home/morse/projects
install -m 755 "$binary" /usr/local/bin/morse
install -m 644 "$root/morse.service" /etc/systemd/system/morse.service
if [[ ! -f /etc/morse.env ]]; then
  umask 077
  { printf 'MORSE_TOKEN=%s\n' "$(openssl rand -hex 32)"; printf 'MORSE_PROVIDER=demo\n'; } > /etc/morse.env
fi
chmod 600 /etc/morse.env
systemctl daemon-reload
systemctl enable --now morse
curl --fail --retry 10 --retry-connrefused --retry-delay 1 http://127.0.0.1:7800/healthz
printf '\nInstalled. Configure /etc/morse.env, then systemctl restart morse. Logs: journalctl -u morse -f\n'
