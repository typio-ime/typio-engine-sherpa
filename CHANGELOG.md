# Changelog

## v0.4.0 - 2026-10-29

### Breaking

- **The legacy in-process C ABI implementation is gone.** `src/sherpa.c`,
  `src/worker_main.c`, `meson.build`, and `typio-engine-sherpa.toml.in` are
  deleted. They could not build: `meson` demanded `typio-engine-abi`, a
  pkg-config module that no longer exists now that Typio's runtime boundary is
  the Engine Protocol process channel. See Typio ADR-0054.

- **The build is Cargo-only.** `cargo build --release` produces the engine.
  sherpa-onnx is discovered through `pkg-config` and bound by the in-tree
  `sherpa-onnx-sys` crate. There is no `meson`, no `libtypio`, and no
  `libcurl`/`libcrypto` dependency on the recogniser path.

### Fixed

- **Voice input now actually transcribes.** The previous `process_audio` returned
  the empty string unconditionally — for any input, including loud audio — while
  reporting `Availability::Ready`. Measured: 1.5 s of audio at peak amplitude 0.60
  produced `Text("")`. The engine now runs sherpa-onnx offline recognition and
  returns the transcript (verified: `"Yeah."` for a generated tone).

- **`Availability` is honest.** `Ready` is reported only when a model directory
  actually resolves. A missing or unloadable model reports `Failed`, and every
  inference failure returns `ReplyRecord::Error`. The `Ready` + `Text("")`
  combination is now unreachable.

- **Silence and undecodable audio are errors, not empty successes.** A payload
  below the noise floor, a payload that is not a multiple of the 4-byte sample
  width, and non-finite samples each produce a specific error message.

- **Model discovery matches the installer's layout.** The engine resolves
  `<data-dir>/sherpa-onnx/<model-name>/`, which is where
  `scripts/install-sensevoice-model.sh` places a model. The previous code looked
  only at `<data-dir>/sherpa-onnx/` and never descended into the model
  subdirectory. A base directory offering more than one candidate model is
  rejected rather than guessed at.

- **`ReloadConfig` drops the loaded recognizer**, so a changed model path or
  language takes effect instead of being silently ignored.

### Added

- **`sherpa-onnx-sys`**: a minimal FFI crate binding exactly eight functions and
  seven types from the sherpa-onnx C API. The allowlist is the contract: an
  upstream rename fails the build at a short declaration block instead of
  surfacing as an undefined symbol at runtime.

- **A `native` feature (default on)** so the pure logic — model discovery, wire
  decoding, availability policy — compiles and tests with no native library
  present. `cargo test --no-default-features` is the portable gate.

- **15 unit tests** covering model-layout resolution (direct, nested, int8
  preference, float fallback, ambiguity rejection, unrelated directories),
  wire decoding (empty, unaligned, endianness), and the availability/silence
  policies. All pass without sherpa-onnx installed.

- **CI split into a pure gate and a native gate.** The native gate installs a
  real model and requires `typio-engine-check` to observe a non-empty
  transcript, because the recorded defect was a green conformance run over a
  stub.

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
