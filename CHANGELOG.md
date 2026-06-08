# Changelog

## Unreleased

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
