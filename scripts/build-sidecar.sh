#!/bin/sh
# Build the /etc/hosts helper and stage it as a Tauri sidecar.
#
# Tauri resolves externalBin entries by appending the target triple, so the
# staged file must be named `portico-hostsd-<triple>`. Without this the app
# bundles without a helper and every new domain prompts for a password.
set -e

cd "$(dirname "$0")/.."
TRIPLE=$(rustc -vV | awk '/^host:/ {print $2}')
OUT="app/src-tauri/binaries"

cargo build --release -p portico-hostsd
mkdir -p "$OUT"
cp "target/release/portico-hostsd" "$OUT/portico-hostsd-$TRIPLE"
chmod 755 "$OUT/portico-hostsd-$TRIPLE"

echo "staged $OUT/portico-hostsd-$TRIPLE"
