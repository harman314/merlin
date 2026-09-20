#!/usr/bin/env bash
# Builds, signs with the local identity, and runs.
#
# cargo run would replace the binary and drop the signature, so the build and
# the signing have to happen before the launch rather than around it.
set -euo pipefail

cd "$(dirname "$0")/.."

NAME="${MERLIN_DEV_IDENTITY:-Merlin Dev}"
PROFILE="${1:-debug}"
shift || true

if [ "$PROFILE" = "release" ]; then
    cargo build --release
    binary="target/release/merlin"
else
    cargo build
    binary="target/debug/merlin"
fi

if [ "$(uname -s)" = "Darwin" ]; then
    if security find-certificate -c "$NAME" >/dev/null 2>&1; then
        codesign --force --entitlements packaging/macos/entitlements.plist \
            --sign "$NAME" "$binary"
    else
        echo "No \"$NAME\" identity; run scripts/dev-identity.sh to stop the" >&2
        echo "keychain asking on every build." >&2
    fi
fi

exec "$binary" "$@"
