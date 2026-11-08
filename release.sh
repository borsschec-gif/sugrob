#!/bin/bash
# Выпускает версию из файла VERSION: универсальная сборка, тег и релиз на GitHub.
# У всех установленных копий приложение обновится само в течение 6 часов.
# Использование: подними VERSION, закоммить изменения и запусти ./release.sh "что нового"
set -euo pipefail
cd "$(dirname "$0")"

VERSION=$(cat VERSION)
NOTES=${1:-"Версия $VERSION"}

if ! security find-certificate -c "Sugrob Dev" >/dev/null 2>&1; then
    echo "Нет сертификата Sugrob Dev: релиз с другой подписью не обновит установленные копии" >&2
    exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
    echo "Есть незакоммиченные изменения" >&2
    exit 1
fi

UNIVERSAL=1 ./build.sh
rm -f build/Sugrob.zip
ditto -c -k --keepParent build/Sugrob.app build/Sugrob.zip
./make-dmg.sh

git push
gh release create "v$VERSION" build/Sugrob.zip build/Sugrob.dmg --title "Сугроб $VERSION" --notes "$NOTES"
echo "Выпущено: v$VERSION"
