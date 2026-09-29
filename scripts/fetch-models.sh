#!/usr/bin/env bash
# Download laya (the Kev inference server) and the Kev-4B GGUF into $KRITIC_HOME.
#
#   scripts/fetch-models.sh [--backend cuda-sm89|cuda-sm86|cuda-sm80|vulkan|metal] [--quant q8_0|ud_q4_k_m|f16]
#
# Sources: laya from https://github.com/monatis/ggmlc/releases (Apache-2.0),
# Kev-4B from https://huggingface.co/mys/kev-4b-GGUF (Apache-2.0; model: https://github.com/jaredpalmer/kev).
# The vision model is separate: install Ollama, then `ollama pull qwen2.5vl:7b`.
set -euo pipefail
HOME_DIR="${KRITIC_HOME:-$HOME/.kritic}"
BACKEND="" QUANT=q8_0
while [ $# -gt 0 ]; do
  case "$1" in
    --backend) BACKEND="$2"; shift ;;
    --quant) QUANT="$2"; shift ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
  shift
done
case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) OS=windows EXT=zip ;;
  Darwin) OS=macos EXT=tar.gz; BACKEND="${BACKEND:-metal}" ;;
  *) OS=linux EXT=tar.gz ;;
esac
if [ -z "$BACKEND" ]; then
  # sm89 PTX JIT-compiles for newer GPUs (measured on an RTX 5070, sm_120); vulkan works everywhere else.
  if command -v nvidia-smi >/dev/null; then BACKEND=cuda-sm89; else BACKEND=vulkan; fi
fi
ARCH=x86_64; [ "$OS" = macos ] && ARCH=arm64
ASSET="laya-$OS-$ARCH-$BACKEND.$EXT"
mkdir -p "$HOME_DIR/laya" "$HOME_DIR/models"
echo "laya: $ASSET"
curl -fL --progress-bar -o "$HOME_DIR/laya/$ASSET" "https://github.com/monatis/ggmlc/releases/latest/download/$ASSET"
if [ "$EXT" = zip ]; then (cd "$HOME_DIR/laya" && unzip -oq "$ASSET"); else tar -xzf "$HOME_DIR/laya/$ASSET" -C "$HOME_DIR/laya"; fi
rm -f "$HOME_DIR/laya/$ASSET"
LAYA="$(find "$HOME_DIR/laya" -type f \( -name laya -o -name laya.exe \) | head -1)"
[ -n "$LAYA" ] || { echo "laya executable not found in the archive" >&2; exit 1; }
chmod +x "$LAYA" 2>/dev/null || true
if [ "$OS" = windows ] && [[ "$BACKEND" == cuda-* ]]; then
  # laya.exe needs the CUDA 12 runtime and cuBLAS DLLs beside it; NVIDIA ships them as wheels.
  TMP="$(mktemp -d)"
  python -m pip download --quiet --no-deps --only-binary=:all: --platform win_amd64 -d "$TMP"     "nvidia-cublas-cu12==12.9.*" "nvidia-cuda-runtime-cu12==12.9.*"
  for w in "$TMP"/*.whl; do unzip -oqj "$w" 'nvidia/*/bin/*.dll' -d "$(dirname "$LAYA")"; done
  rm -rf "$TMP"
  echo "CUDA runtime DLLs -> $(dirname "$LAYA")"
fi
GGUF="kev_4b_$QUANT.gguf"
echo "Kev-4B: $GGUF"
curl -fL --progress-bar -C - -o "$HOME_DIR/models/$GGUF" "https://huggingface.co/mys/kev-4b-GGUF/resolve/main/$GGUF"
cat <<MSG

laya:  $LAYA
Kev:   $HOME_DIR/models/$GGUF
If these differ from the defaults, set them in $HOME_DIR/kritic.toml:
  [kev]
  laya_exe = "$LAYA"
  gguf = "$HOME_DIR/models/$GGUF"
MSG
