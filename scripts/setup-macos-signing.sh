#!/usr/bin/env bash
# One-time setup: creates a local self-signed code-signing identity ("Local Notebook Signing") so that every
# build of the macOS app is signed with the SAME identity.
#
# Why: with an ad-hoc signature (`codesign --sign -`) the app's identity is just its code hash, which changes on
# every rebuild, so macOS asks for the login password again before the app may read its Keychain items.
# A fixed certificate gives the app a stable identity, so "Always Allow" survives rebuilds and upgrades.
#
# Usage: bash scripts/setup-macos-signing.sh [--keychain PATH] [--backup PATH]
#   --keychain  keychain to put the identity in (default: your login keychain)
#   --backup    where to write the PKCS#12 backup (default: ~/LocalNotebookSigning.p12)
#
# KEEP THE BACKUP. Releases signed with a different certificate get a different identity, and every user is
# asked for the Keychain password once on upgrade.
set -euo pipefail

NAME="${LNA_SIGN_IDENTITY:-Local Notebook Signing}"
KEYCHAIN=""
BACKUP="$HOME/LocalNotebookSigning.p12"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --keychain) KEYCHAIN="$2"; shift 2 ;;
    --backup) BACKUP="$2"; shift 2 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
done
if [[ -z "$KEYCHAIN" ]]; then
  KEYCHAIN="$(security default-keychain -d user | tr -d ' "')"
fi

if security find-identity -p codesigning "$KEYCHAIN" 2>/dev/null | grep -q "\"$NAME\""; then
  echo "Signing identity \"$NAME\" already exists in $KEYCHAIN - nothing to do."
  exit 0
fi

# LibreSSL (/usr/bin/openssl) writes a PKCS#12 that `security import` understands; OpenSSL 3 needs -legacy.
OPENSSL=/usr/bin/openssl
[[ -x "$OPENSSL" ]] || OPENSSL="$(command -v openssl)"
P12_FLAGS=()
if "$OPENSSL" version | grep -q "^OpenSSL 3"; then P12_FLAGS=(-legacy); fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
chmod 700 "$WORK"

cat > "$WORK/openssl.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CNF

echo "Creating the self-signed code-signing certificate \"$NAME\" (valid 20 years)..."
"$OPENSSL" req -x509 -newkey rsa:2048 -nodes -days 7300 -config "$WORK/openssl.cnf" \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" 2>/dev/null

PASS="$("$OPENSSL" rand -hex 16)"
"$OPENSSL" pkcs12 -export ${P12_FLAGS[@]+"${P12_FLAGS[@]}"} -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -name "$NAME" -passout "pass:$PASS" -out "$WORK/identity.p12"

umask 077
cp "$WORK/identity.p12" "$BACKUP"
chmod 600 "$BACKUP"

echo "Importing into $KEYCHAIN (codesign is allowed to use the key)..."
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$PASS" -T /usr/bin/codesign >/dev/null

if [[ "${LNA_SKIP_TRUST:-0}" != "1" ]]; then
  echo "Trusting the certificate for code signing (macOS may ask for your password)..."
  security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$WORK/cert.pem" \
    || echo "Warning: could not set trust; signing can still work. Open Keychain Access > $NAME > Trust > Code Signing: Always Trust."
fi

cat <<MSG

Done. Backup of the identity: $BACKUP
Backup password (shown once, store it with the file): $PASS

Build with it:  bash scripts/package-macos-app.sh
The first signature may ask whether codesign may use the key: choose "Always Allow".
MSG
