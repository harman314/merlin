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

if security find-identity -v -p codesigning | grep -qF "$NAME"; then
    echo "\"$NAME\" already exists. Build with scripts/dev-run.sh."
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

# The key and certificate go in separately. A PKCS#12 bundle would be tidier,
# but OpenSSL 3 writes one macOS cannot read, and which openssl is on PATH is
# not ours to decide. The keychain pairs them by public key.
security import "$work/key.pem" -k "$KEYCHAIN" -T /usr/bin/codesign >/dev/null
security import "$work/cert.pem" -k "$KEYCHAIN" -T /usr/bin/codesign >/dev/null

# codesign rejects a certificate it does not trust for signing code.
echo "Trusting the certificate for code signing needs your admin password."
sudo security add-trusted-cert -d -r trustRoot \
    -p codeSign -k /Library/Keychains/System.keychain "$work/cert.pem"

if security find-identity -v -p codesigning | grep -qF "$NAME"; then
    echo
    echo "Created \"$NAME\"."
    echo "Build and run with: scripts/dev-run.sh"
    echo "The first launch asks for the keychain once. Choose Always Allow."
else
    echo
    echo "The identity did not register. codesign cannot see \"$NAME\"." >&2
    echo "Check: security find-identity -v -p codesigning" >&2
    exit 1
fi
