# How to Install a Speech Model

The worker ships no model. It loads one at focus-in time from the host's data
directory. Follow these steps to make a model available.

## How config maps to files

Two config keys drive loading:

| Config key | What it selects | Example value |
| --- | --- | --- |
| `voice.engine` | Which voice engine to activate | `sherpa-onnx` |
| `engines.sherpa-onnx.model` | Which model directory the engine uses | `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17` |

Loading chain:

```
voice.engine = "sherpa-onnx"
    → host reads typio-engine-sherpa.toml and starts typio-engine-sherpa

engines.sherpa-onnx.model = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
    → engine opens <data-dir>/sherpa-onnx/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/
    → detects model type from directory contents
```

`<data-dir>` is the host's instance data directory (commonly
`~/.local/share/typio/`).

## Quick install (SenseVoice Small)

```sh
scripts/install-sensevoice-model.sh
```

Downloads the recommended int8 model from
[sherpa-onnx releases](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models)
and installs **only the model files** (`model.int8.onnx`, `tokens.txt`) under
`<data-dir>/sherpa-onnx/<upstream-name>/`.

The upstream directory name is used as-is in config:

| Upstream tarball | Directory on disk | Config `model` value |
| --- | --- | --- |
| `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2` | `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/` | `sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17` |

Script options:

| Flag | Effect |
| --- | --- |
| `-d DIR` | Override target data directory |
| `-v VER` | Model version tag (default: `2024-07-17`) |
| `-f` | Force re-download |

## Manual install

### 1. Download

Download a non-streaming (offline) model from the
[sherpa-onnx pre-trained models](https://github.com/k2-fsa/sherpa-onnx/releases/tag/asr-models).
SenseVoice, Whisper, transducer, and Paraformer models are all supported.

Example — SenseVoice Small int8:

```sh
wget https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
tar xf sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2
```

### 2. Place it under the data directory

Copy **only the model files** into a subdirectory under
`<data-dir>/sherpa-onnx/`, using the upstream name:

```sh
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/typio/sherpa-onnx/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
mkdir -p "$DEST"
cp sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/{model.int8.onnx,tokens.txt} "$DEST/"
```

Resulting layout:

```
<data-dir>/sherpa-onnx/
└── sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17/
    ├── model.int8.onnx
    └── tokens.txt
```

The directory name matters for SenseVoice models: include `sense-voice`,
`sense_voice`, or `sensevoice` in it so the engine selects the SenseVoice path
rather than treating the model as a Paraformer. See
[Configuration](../reference/configuration.md) for the full detection rules.

### 3. Pin the model

With one model directory present, the engine auto-detects it. With several,
select one explicitly using the upstream directory name:

```toml
voice.engine = "sherpa-onnx"

[engines.sherpa-onnx]
model = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17"
language = "auto"
```

### 4. Verify

Focus a text field with the engine active. The host logs the detection result,
for example:

```
sherpa-onnx: detected model type 'sense_voice' in <data-dir>/sherpa-onnx/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17
sherpa-onnx: recognizer created (type=sense_voice, dir=...)
```

If you instead see `sherpa-onnx: no recognizable model in ...`, the files are
missing or nested too deep — the model files must be exactly one level under
`sherpa-onnx/`.
