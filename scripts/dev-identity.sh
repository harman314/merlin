#!/usr/bin/env bash
# Creates a self-signed code-signing identity so macOS stops asking for the
# keychain password on every build.
#
# macOS ties keychain access to an app's signing identity. An unsigned binary,
# which is what cargo produces, gets a new identity every compile, so "Always
# Allow" never sticks. Signing every build with one stable identity fixes that.
#
# Run once. Then build with scripts/dev-run.sh.
set -euo pipefail

NAME="${1:-Merlin Dev}"
KEYCHAIN="$HOME/Library/Keychains/login.keychain-db"

if [ "$(uname -s)" != "Darwin" ]; then
    echo "macOS only; other platforms do not gate the keyring on a signature." >&2
    exit 1
fi

if security find-certificate -c "$NAME" >/dev/null 2>&1; then
    echo "\"$NAME\" already exists. Nothing to do."
    exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# codeSigning in extendedKeyUsage is what makes codesign accept the identity.
openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
    -keyout "$work/key.pem" -out "$work/cert.pem" \
    -subj "/CN=$NAME" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning" >/dev/null 2>&1

openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
    -out "$work/identity.p12" -passout pass: >/dev/null 2>&1

# -T lets codesign use the key without prompting for it separately.
security import "$work/identity.p12" -k "$KEYCHAIN" -P "" \
    -T /usr/bin/codesign -T /usr/bin/security >/dev/null

# Without trust, codesign rejects the certificate as unsuitable.
echo "Trusting the certificate for code signing needs your admin password."
sudo security add-trusted-cert -d -r trustRoot \
    -p codeSign -k /Library/Keychains/System.keychain "$work/cert.pem"

# Stops the keychain prompting again the first time codesign reads the key.
security set-key-partition-list -S apple-tool:,apple: -s -k "" "$KEYCHAIN" >/dev/null 2>&1 || true

echo
echo "Created \"$NAME\"."
echo "Build and run with: scripts/dev-run.sh"
echo "The first launch still asks for the keychain once. Choose Always Allow."
