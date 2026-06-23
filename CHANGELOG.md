# Changelog

## Unreleased

- **Engine info aligned with manifest.** `sherpa_info.icon` now reports
  `typio-sherpa-symbolic` (was the stale `typio-sherpa`) and
  `sherpa_info.description` now reads "Voice input engine powered by
  sherpa-onnx." — both matching the values the manifest already ships. Hosts
  read metadata from the manifest, so this is cosmetic when queried via the
  C ABI, but keeps the two sources of truth in agreement.
- **`sherpa_base_ops` now provides `on_config_change`.** Matches the
  `TypioEngineBaseOps` contract documented in
  `libtypio include/typio/abi/engine.h` (all callbacks must be present;
  supply a no-op when the behaviour is not needed). The implementation is a
  no-op: the only configured values are model paths, which `reload_config`
  already handles via a full reload.
- Stale comment in `meson.build` updated to reference the installed
  `typio-sherpa-symbolic` icon name.

## v0.2.1 - 2026-06-19

- `worker_main.c` is now byte-identical to the shared harness used by the
  other engine workers. Adds the `salience` field to MODE/ACTIVE_MODE
  lines, an `ACTIVE_MODE` line after every mode-affecting request, and
  the safer teardown ordering (deactivate → instance → engine).

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
