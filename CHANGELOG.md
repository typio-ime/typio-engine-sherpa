# Changelog

## Unreleased

## v0.2.0 - 2026-06-13

- Declare supported languages in the manifest (`languages = ["mul"]`) for
  language-first switching (typio-linux ADR-0031).
- worker_main: derive the protocol hello engine type from
  `TypioEngineInfo.type` instead of hard-coding "voice", keeping the
  harness copy-paste safe for keyboard engines.
- Centralize recognizer loading in `ensure_model_loaded` and reload the
  model immediately after a config change instead of waiting for the
  next focus-in.

## v0.1.0 - 2026-06-06

- Build Sherpa-ONNX as a direct engine executable with a TOML manifest.
- Install the private worker under `<libexecdir>/typio/engines` and the
  manifest under `<datadir>/typio/engines`.
- Export the engine config schema before local instance initialization.

## v0.0.4

- Migrate voice readiness from `TypioVoiceEngineOps::is_ready` to
  `TypioEngineBaseOps::availability`.

## v0.0.3

- Fix `snprintf` truncation warning in `extract_files` by enlarging path buffers.

## v0.0.2

- Rename install-model command to setup, register config schema, and auto-write model config.
- Fix setup running in background thread to avoid blocking IPC.

## v0.0.1

- Initial release.
