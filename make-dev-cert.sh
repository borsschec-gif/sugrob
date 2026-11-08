#!/bin/bash
# Один раз создаёт самоподписанный сертификат «Sugrob Dev» в связке входа.
# С постоянной подписью разрешение Accessibility не слетает при каждой сборке.
# Удалить: security delete-identity -c "Sugrob Dev"
set -euo pipefail

NAME="Sugrob Dev"
if security find-certificate -c "$NAME" >/dev/null 2>&1; then
    echo "Сертификат уже есть"
    exit 0
fi

DIR=$(mktemp -d)
trap 'rm -rf "$DIR"' EXIT

cat > "$DIR/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -config "$DIR/cert.cnf" \
    -keyout "$DIR/key.pem" -out "$DIR/cert.pem" 2>/dev/null
openssl pkcs12 -export -inkey "$DIR/key.pem" -in "$DIR/cert.pem" \
    -out "$DIR/cert.p12" -passout pass:sugrob
security import "$DIR/cert.p12" -k "$HOME/Library/Keychains/login.keychain-db" -P sugrob -A
echo "Готово: $NAME"
