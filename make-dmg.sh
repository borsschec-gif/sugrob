#!/bin/bash
# Собирает build/Sugrob.dmg из готового build/Sugrob.app.
set -euo pipefail
cd "$(dirname "$0")"

STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT
ditto build/Sugrob.app "$STAGE/Sugrob.app"
ln -s /Applications "$STAGE/Программы"
rm -f build/Sugrob.dmg
hdiutil create -quiet -volname "Сугроб" -srcfolder "$STAGE" -ov -format UDZO build/Sugrob.dmg
