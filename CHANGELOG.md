# Changelog

## Unreleased

- Migrate voice readiness from `TypioVoiceEngineOps::is_ready` to
  `TypioEngineBaseOps::availability`.

## v0.0.3

- Fix `snprintf` truncation warning in `extract_files` by enlarging path buffers.

## v0.0.2

- Rename install-model command to setup, register config schema, and auto-write model config.
- Fix setup running in background thread to avoid blocking IPC.

## v0.0.1

- Initial release.
