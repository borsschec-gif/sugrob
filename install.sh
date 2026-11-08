#!/bin/bash
# Устанавливает Сугроб в /Applications. Запуск:
# curl -fsSL https://raw.githubusercontent.com/borsschec-gif/sugrob/main/install.sh | bash
set -euo pipefail

URL="https://github.com/borsschec-gif/sugrob/releases/latest/download/Sugrob.zip"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "Скачиваю Сугроб…"
curl -fsSL "$URL" -o "$TMP/Sugrob.zip"
ditto -x -k "$TMP/Sugrob.zip" "$TMP"

pkill -x Sugrob 2>/dev/null || true
rm -rf /Applications/Sugrob.app
mv "$TMP/Sugrob.app" /Applications/Sugrob.app
open /Applications/Sugrob.app
echo "Готово. В Настройках разреши Sugrob: Универсальный доступ и Запись экрана."
