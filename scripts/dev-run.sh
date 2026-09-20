#!/usr/bin/env bash
# Builds, signs with the local identity, and runs.
#
# cargo run would replace the binary and drop the signature, so the build and
# the signing have to happen before the launch rather than around it.
set -euo pipefail

cd "$(dirname "$0")/.."

NAME="${MERLIN_DEV_IDENTITY:-Merlin Dev}"

# Run as Merlin Dev, a separate app with its own directories, instance port
# and linked device, so this build and an installed Merlin run at once. Set
# MERLIN_PROFILE=installed to work on the installed app's own data instead.
export MERLIN_PROFILE="${MERLIN_PROFILE:-dev}"

# Only a literal profile is consumed. Everything else, --verbose included,
# belongs to the app.
PROFILE=debug
case "${1:-}" in
    debug | release)
        PROFILE="$1"
        shift
        ;;
esac

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
