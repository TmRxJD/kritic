#!/usr/bin/env bash
# Build kritic and install it into $KRITIC_HOME (default ~/.kritic):
#   bin/kritic[.exe]   the MCP server
#   bin/harness/       optional desktop/Python harnesses; web checks use native CDP (found beside the binary)
#   kritic.toml        created from examples/kritic.toml if absent
#
#   scripts/install.sh [--no-harness] [--with-models] [--backend <laya backend>]
#
# --with-models runs scripts/fetch-models.sh (laya + Kev-4B GGUF, ~4.5 GB).
# Windows locks a running exe: if a session is running the server, the old binary is renamed
# aside so running sessions keep it and new sessions get the new build.
set -euo pipefail
cd "$(dirname "$0")/.."
HOME_DIR="${KRITIC_HOME:-$HOME/.kritic}"
HARNESS=1 MODELS=0 BACKEND_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --no-harness) HARNESS=0 ;;
    --with-models) MODELS=1 ;;
    --backend) BACKEND_ARGS=(--backend "$2"); shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
EXE=kritic; case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) EXE=kritic.exe ;; esac

cargo build --release --quiet
TARGET="$(cargo metadata --format-version 1 --no-deps | python -c 'import sys,json;print(json.load(sys.stdin)["target_directory"])')"
mkdir -p "$HOME_DIR/bin" "$HOME_DIR/logs" "$HOME_DIR/runs"
DEST="$HOME_DIR/bin/$EXE"
rm -f "$DEST".old-* 2>/dev/null || true
if ! cp "$TARGET/release/$EXE" "$DEST" 2>/dev/null; then
  mv "$DEST" "$DEST.old-$(date +%s)" && cp "$TARGET/release/$EXE" "$DEST"
  echo "note: a running session holds the previous build; restart it to pick this one up"
fi
echo "installed $DEST"

if [ "$HARNESS" = 1 ]; then
  rm -rf "$HOME_DIR/bin/harness"
  mkdir -p "$HOME_DIR/bin/harness"
  (cd harness && tar --exclude=node_modules --exclude=.venv --exclude=__pycache__ --exclude=out -cf - .) | (cd "$HOME_DIR/bin/harness" && tar -xf -)
  if command -v uv >/dev/null; then
    (cd "$HOME_DIR/bin/harness/py" && uv sync --quiet)
  else
    echo "note: uv not found; optional desktop_test needs it; resource/hygiene checks run natively (https://docs.astral.sh/uv/)"
  fi
  echo "installed harness -> $HOME_DIR/bin/harness"
fi

if [ ! -f "$HOME_DIR/kritic.toml" ]; then
  cp examples/kritic.toml "$HOME_DIR/kritic.toml"
  echo "wrote $HOME_DIR/kritic.toml (every setting commented with its default)"
fi

if [ "$MODELS" = 1 ]; then
  KRITIC_HOME="$HOME_DIR" scripts/fetch-models.sh "${BACKEND_ARGS[@]}"
  echo "models downloaded; explicitly select review.backend = local to enable inference"
fi

SHOWN="$DEST"; command -v cygpath >/dev/null && SHOWN="$(cygpath -m "$DEST")"
cat <<MSG

Register it with Claude Code (user scope, every project):
  claude mcp add --scope user kritic -- "$SHOWN"
Then start a new session and call status(start=false). Models are off by default.
MSG
