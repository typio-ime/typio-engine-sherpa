# Changelog

## Unreleased

- **Pinned upstream compatibility CI.** The production build now compiles the
  worker with warnings as errors against a fixed sherpa-onnx C API revision
  and verifies its installed worker, manifest, and icon layout.
- **ABI compatibility gate.** The worker validates the engine's exported ABI
  version before reading schema metadata or constructing model state.
- **Schema-first worker handshake.** The shared worker publishes Sherpa's
  config schema before model construction, tolerates host discovery probes
  that close before HostHello, and validates the required voice operation.
- **Setup command is reachable and safe across worker shutdown.** Typio Engine
  Protocol now exposes the engine's `setup` command. The background installer
  is single-flight, cancellable, and joined during teardown, preventing its
  engine pointer from outliving `SherpaState` or delaying shutdown for the
  full network timeout. A completed setup is loaded during the next
  availability check, while later workers find it through normal model
  auto-detection.
- **Model installation rejects truncated paths and unsafe extraction state.**
  Path joins now fail explicitly instead of using truncated buffers, downloads
  handle partial writes, use HTTPS-only redirects and bounded transfer
  timeouts, and verify the pinned release asset's SHA-256 digest. Archives
  extract into a private temporary directory, and incomplete model directories
  are repaired instead of treated as installed.

## v0.2.2 - 2026-06-23

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
