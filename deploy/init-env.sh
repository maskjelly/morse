#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
[[ ! -e .env ]] || { echo '.env already exists; left unchanged.'; exit 0; }
command -v openssl >/dev/null || { echo 'Install openssl first.' >&2; exit 1; }
umask 077
(set -o noclobber; sed "s/replace-with-openssl-rand-hex-32/$(openssl rand -hex 32)/" deploy/morse.env.example > .env)
echo 'Created private .env with a random token. Configure your provider, then docker compose up --build -d.'
