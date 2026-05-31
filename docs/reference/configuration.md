# Configuration Reference

## Config keys

Read from the host's unified config tree under `engines.sherpa-onnx.*`.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `engines.sherpa-onnx.model` | string | `""` (auto-detect) | Subdirectory under `<data-dir>/sherpa-onnx/` to load. When empty, the first subdirectory containing a recognizable model is used. |
| `engines.sherpa-onnx.language` | string | `""` | Recognition language hint. Interpreted per model type (see below). |

## Model discovery

| Step | Behavior |
| --- | --- |
| Base directory | `<instance-data-dir>/sherpa-onnx/` |
| With `model` set | Loads `<base>/<model>`. |
| With `model` empty | Scans `<base>` and loads the first subdirectory whose files match a known type. |
| Load timing | Lazy — on `focus_in`. Freed on `deactivate`. |

## Model type detection

Detection runs in this order; the first match wins. File names match an exact
name or a `*-<suffix>` fallback.

| Type | Required files | Extra condition |
| --- | --- | --- |
| SenseVoice | `tokens.txt`, `model.int8.onnx` or `model.onnx` | Directory name contains `sense-voice`, `sense_voice`, or `sensevoice`. |
| Paraformer | `tokens.txt`, `model.int8.onnx` or `model.onnx` | Directory name does not match SenseVoice. |
| Transducer | `tokens.txt`, `encoder.onnx`, `decoder.onnx`, `joiner.onnx` | — |
| Whisper | `tokens.txt`, `encoder.onnx`, `decoder.onnx` | No `joiner.onnx` present. |

## Language handling

| Type | Use of `language` |
| --- | --- |
| SenseVoice | Passed through; empty value becomes `auto`. ITN (inverse text normalization) is enabled. |
| Whisper | Passed through; empty value lets Whisper auto-detect. Task is `transcribe`. |
| Transducer | Ignored. |
| Paraformer | Ignored. |

## Fixed runtime parameters

Not configurable; set by the engine.

| Parameter | Value |
| --- | --- |
| Sample rate | 16000 Hz |
| Feature dimension | 80 |
| Channels | mono |
| Sample format | PCM float32 |
| Threads | 4 |
| Provider | `cpu` |
| Decoding method | `greedy_search` |
