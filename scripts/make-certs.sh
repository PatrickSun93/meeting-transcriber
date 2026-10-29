#!/usr/bin/env bash
# Creates a trusted HTTPS certificate for localhost in ./certs.
# Teams only loads tabs over HTTPS with a certificate your Mac trusts.
set -euo pipefail

cd "$(dirname "$0")/.."
mkdir -p certs

if ! command -v mkcert >/dev/null 2>&1; then
  echo "mkcert is not installed. On macOS run:"
  echo "  brew install mkcert"
  echo "then run this script again."
  exit 1
fi

# Adds mkcert's local CA to the macOS keychain (asks for your password once).
mkcert -install
mkcert -key-file certs/localhost-key.pem -cert-file certs/localhost.pem localhost 127.0.0.1 ::1
echo "Certificates written to ./certs. Restart the server with: npm start"
