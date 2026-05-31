#!/usr/bin/env bash
set -euo pipefail

MODEL_VERSION="2024-07-17"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/typio/sherpa-onnx"
UPSTREAM_NAME="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-${MODEL_VERSION}"
URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${UPSTREAM_NAME}.tar.bz2"

usage() {
    cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Download and install the SenseVoice Small int8 model for typio-engine-sherpa.

Installs to:  <data-dir>/sherpa-onnx/${UPSTREAM_NAME}/
Config key:   model = "${UPSTREAM_NAME}"

Options:
    -d DIR   Override target data directory (default: ${DATA_DIR})
    -v VER   Model version tag (default: ${MODEL_VERSION})
    -f       Force re-download even if model exists
    -h       Show this help
EOF
}

FORCE=0

while getopts "d:v:fh" opt; do
    case "$opt" in
        d) DATA_DIR="$OPTARG" ;;
        v)
            MODEL_VERSION="$OPTARG"
            UPSTREAM_NAME="sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-${MODEL_VERSION}"
            URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/${UPSTREAM_NAME}.tar.bz2"
            ;;
        f) FORCE=1 ;;
        h) usage; exit 0 ;;
        *) usage; exit 1 ;;
    esac
done

TARGET_DIR="${DATA_DIR}/${UPSTREAM_NAME}"

if [ -d "$TARGET_DIR" ] && [ "$FORCE" -eq 0 ]; then
    echo "Model already installed at ${TARGET_DIR}"
    echo "Use -f to force re-download."
    exit 0
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

echo "Downloading ${UPSTREAM_NAME}.tar.bz2 ..."
curl -fSL -o "${TMPDIR}/${UPSTREAM_NAME}.tar.bz2" "$URL"

echo "Extracting model files..."
tar xf "${TMPDIR}/${UPSTREAM_NAME}.tar.bz2" -C "$TMPDIR"

mkdir -p "$TARGET_DIR"
cp "${TMPDIR}/${UPSTREAM_NAME}/model.int8.onnx" "$TARGET_DIR/"
cp "${TMPDIR}/${UPSTREAM_NAME}/tokens.txt" "$TARGET_DIR/"

echo "Installed to ${TARGET_DIR}"
ls -lh "$TARGET_DIR"
