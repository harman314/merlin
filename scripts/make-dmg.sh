#!/usr/bin/env bash
# Build an installable Merlin.dmg on this Mac.
#
#   scripts/make-dmg.sh [version]
#
# The app is signed with the local "Merlin Dev" identity when one exists, and
# ad-hoc otherwise. Neither is a Developer ID, so this DMG is for your own
# Macs. Anything downloaded from the web carries a quarantine flag that
# Gatekeeper refuses for an unsigned app.
set -euo pipefail

cd "$(dirname "$0")/.."

if [ "$(uname -s)" != "Darwin" ]; then
    echo "A macOS DMG can only be built on macOS." >&2
    exit 1
fi

NAME="${MERLIN_DEV_IDENTITY:-Merlin Dev}"
VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
OUTPUT="dist/Merlin-$VERSION.dmg"

cargo build --release

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

packaging/macos/bundle.sh target/release/merlin "$stage/Merlin.app" "$VERSION" >/dev/null

# bundle.sh signs ad-hoc, which mints a new identity on every build and makes
# the keychain ask again. The dev certificate keeps one identity across builds.
if security find-certificate -c "$NAME" >/dev/null 2>&1; then
    codesign --force --entitlements packaging/macos/entitlements.plist \
        --sign "$NAME" "$stage/Merlin.app"
else
    echo "No \"$NAME\" identity. Run scripts/dev-identity.sh to stop the" >&2
    echo "keychain asking after every rebuild." >&2
fi
codesign --verify --strict "$stage/Merlin.app"

# The symlink is what lets you drag the app across in the mounted window.
ln -s /Applications "$stage/Applications"

mkdir -p dist
rm -f "$OUTPUT"
hdiutil create -volname Merlin -srcfolder "$stage" -format UDZO "$OUTPUT" >/dev/null
hdiutil verify "$OUTPUT" >/dev/null

echo "$OUTPUT"
