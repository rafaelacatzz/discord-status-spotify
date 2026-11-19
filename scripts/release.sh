#!/usr/bin/env bash
# Publica uma versão do SpotifySync no GitHub, com o .exe pronto no release.
#
# Uso:
#   ./scripts/release.sh              # compila, cria o release e anexa os arquivos
#   ./scripts/release.sh --no-upload  # apenas compila e empacota em dist/
#
# Pré-requisitos: Rust (cargo), git com credencial do GitHub, curl e python.
# O token usado para publicar é lido de GITHUB_TOKEN, do `gh auth token` ou da
# credencial que o git já usa para dar push — nunca é gravado em disco.
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
[ -n "$VERSION" ] || { echo "Não foi possível ler a versão do Cargo.toml." >&2; exit 1; }
TAG="v$VERSION"
REPO=$(git config --get remote.origin.url |
    sed -E 's#.*github\.com[:/]([^/]+/[^/]+?)(\.git)?$#\1#')
[ -n "$REPO" ] || { echo "Remoto do GitHub não encontrado." >&2; exit 1; }
echo "SpotifySync $VERSION  ->  $REPO ($TAG)"

PY=""
for candidate in python3 python; do
    # The Windows Store alias answers `command -v` but fails when executed.
    if command -v "$candidate" >/dev/null 2>&1 &&
        "$candidate" -c "import sys" >/dev/null 2>&1; then
        PY=$candidate
        break
    fi
done
[ -n "$PY" ] || { echo "python não encontrado (necessário para empacotar e publicar)." >&2; exit 1; }

cargo test --locked
cargo build --release --locked

mkdir -p dist
cp -f target/release/SpotifySync.exe dist/SpotifySync.exe
"$PY" - <<'PY'
import zipfile

with zipfile.ZipFile("dist/SpotifySync-windows-x64.zip", "w", zipfile.ZIP_DEFLATED) as bundle:
    bundle.write("dist/SpotifySync.exe", "SpotifySync.exe")
    bundle.write("README.md", "README.md")
    bundle.write("LICENSE", "LICENSE")
PY
echo "build ok: dist/SpotifySync.exe + dist/SpotifySync-windows-x64.zip"

if [ "${1:-}" = "--no-upload" ]; then
    exit 0
fi

TOKEN="${GITHUB_TOKEN:-}"
if [ -z "$TOKEN" ] && command -v gh >/dev/null 2>&1; then
    TOKEN=$(gh auth token 2>/dev/null || true)
fi
if [ -z "$TOKEN" ]; then
    TOKEN=$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null |
        sed -n 's/^password=//p')
fi
[ -n "$TOKEN" ] || { echo "Nenhuma credencial do GitHub encontrada. Defina GITHUB_TOKEN." >&2; exit 1; }

auth() {
    printf 'header = "Authorization: Bearer %s"\nheader = "Accept: application/vnd.github+json"\n' "$TOKEN"
}

if ! git rev-parse "$TAG" >/dev/null 2>&1; then
    git tag "$TAG"
fi
git push origin main --tags

PAYLOAD=".release-payload.json"
printf '{"tag_name":"%s","name":"SpotifySync %s","draft":false,"prerelease":false,"body":"%s"}' \
    "$TAG" "$VERSION" \
    "Baixe SpotifySync.exe e execute (Windows 10/11 x64). O .zip inclui o executavel, o README e a licenca. Se o Windows avisar, use Mais informacoes > Executar assim mesmo." \
    >"$PAYLOAD"

RESPONSE=$(auth | curl -s -m 60 --config - -X POST -H "Content-Type: application/json" \
    --data-binary @"$PAYLOAD" "https://api.github.com/repos/$REPO/releases")
RELEASE_ID=$(printf '%s' "$RESPONSE" |
    "$PY" -c "import json,sys; d=json.load(sys.stdin); print(d.get('id') or '')")
if [ -z "$RELEASE_ID" ]; then
    RELEASE_ID=$(auth | curl -s -m 60 --config - \
        "https://api.github.com/repos/$REPO/releases/tags/$TAG" |
        "$PY" -c "import json,sys; d=json.load(sys.stdin); print(d.get('id') or '')")
fi
rm -f "$PAYLOAD"
[ -n "$RELEASE_ID" ] || { echo "Não foi possível criar ou encontrar o release $TAG." >&2; exit 1; }

# Substitui um arquivo já anexado (o GitHub recusa nomes duplicados).
remove_asset() {
    local name=$1
    local ids
    ids=$(auth | curl -s -m 60 --config - \
        "https://api.github.com/repos/$REPO/releases/$RELEASE_ID/assets" |
        "$PY" -c "
import json, sys
name = sys.argv[1]
for asset in json.load(sys.stdin):
    if asset['name'] == name:
        print(asset['id'])
" "$name")
    for asset_id in $ids; do
        auth | curl -s -m 60 --config - -X DELETE \
            "https://api.github.com/repos/$REPO/releases/assets/$asset_id" >/dev/null
    done
}

upload() {
    local file=$1
    local name=$2
    remove_asset "$name"
    auth | curl -s -m 900 --config - -X POST -H "Content-Type: application/octet-stream" \
        --data-binary @"$file" \
        "https://uploads.github.com/repos/$REPO/releases/$RELEASE_ID/assets?name=$name" >/dev/null
    echo "anexado: $name ($(wc -c <"$file") bytes)"
}

upload dist/SpotifySync.exe SpotifySync.exe
upload dist/SpotifySync-windows-x64.zip SpotifySync-windows-x64.zip

echo "release: https://github.com/$REPO/releases/tag/$TAG"
