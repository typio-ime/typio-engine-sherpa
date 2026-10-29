# Configuration Reference

## Config keys

Read from the host's unified config tree under `engines.sherpa-onnx.*`.

| Key | Type | Default | Effect |
| --- | --- | --- | --- |
| `engines.sherpa-onnx.model` | string | `""` (auto-detect) | Model directory. An absolute path is used as-is; a relative path is resolved against the data directory, then against `<data-dir>/sherpa-onnx/`. Empty selects `<data-dir>/sherpa-onnx/`. |
| `engines.sherpa-onnx.language` | string | `""` (auto) | Recognition language hint. Empty becomes `auto`, letting the model decide. |

## Model discovery

| Step | Behavior |
| --- | --- |
| Base directory | `<instance-data-dir>/sherpa-onnx/` |
| Direct match | If the target directory holds `model.int8.onnx` (or `model.onnx`) and `tokens.txt`, it is used directly. |
| Nested match | Otherwise the base directory is scanned one level down for a model subdirectory. |
| Ambiguity | More than one candidate subdirectory is a **failure**, not a guess: guessing would silently transcribe with the wrong model. |
| Artifact preference | `model.int8.onnx` is preferred over `model.onnx` when both exist. |
| Load timing | Lazy — the model loads on the first `ProcessAudio`. `Availability` reports `Ready` only when a model directory resolves. |

## Supported model family

Only the SenseVoice offline family is supported; it is what the installer ships
and what `sherpa-onnx-sys` binds.

| Family | Required files | Language handling |
| --- | --- | --- |
| SenseVoice | `tokens.txt`, `model.int8.onnx` or `model.onnx` | `language` is passed through; empty becomes `auto`. Inverse text normalization is enabled. |

Whisper, transducer, and Paraformer directories are *not* recognised. Adding a
family means extending `ALLOWED_TYPES`/`ALLOWED_FUNCTIONS` in
`sherpa-onnx-sys/build.rs` and the config construction in `src/sherpa.rs`.

## Language handling

| Value | Effect |
| --- | --- |
| empty | Becomes `auto`; the model detects the language. |
| `auto` | Model-detected. |
| `zh`, `en`, `ja`, `ko`, `yue` | Explicit hint passed to sherpa-onnx. |

## Failure reporting

Recognition never returns an empty transcript as a success. Each of these
produces `ReplyRecord::Error` with a specific message:

| Condition | Message |
| --- | --- |
| Empty payload | `no audio captured` |
| Payload length not a multiple of 4 | `audio payload length N is not a multiple of the 4-byte sample width` |
| Silence (peak < 1e-4) | `audio is silent (peak amplitude X)` |
| Non-finite samples | `audio payload contains non-finite samples` |
| No resolvable model | `no recognisable model in <dir>: expected model.int8.onnx or model.onnx together with tokens.txt` |
| Recognizer produced nothing | `recognizer produced no text for non-silent audio` |

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
