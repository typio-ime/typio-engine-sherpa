# Changelog

## [Unreleased]

### Added

- **Configurable idle memory eviction.** Introduced `engines.sherpa-onnx.idle_timeout_seconds`
  (default: 1800s / 30m; 0 disables). The engine polls its protocol channel with a
  deadline and unloads the SenseVoice model weights from physical memory after the
  configured inactivity period, reducing resident memory footprint from ~350 MB to
  ~10 MB. Subsequent transcription requests reload the model transparently on demand.
- **On-demand background model acquisition (`setup` task).** Implemented the
  `setup` task over the Engine Protocol. Invocations spawn a detached
  worker thread to download, verify, and unpack the default SenseVoice Small int8
  model into `<data_dir>/sherpa-onnx/` without blocking the protocol event loop
  or exceeding the host's 5-second command timeout.
- **Dynamic model enumeration in schema.** `ModelFiles::list_models` discovers
  all installed models under `<data_dir>/sherpa-onnx/` and populates the
  `engines.sherpa-onnx.model` schema field's `options`, enabling dropdown selection
  in GUI settings.
- **Availability lifecycle reflection.** The engine reports `Availability::Preparing`
  while background provisioning is running and transitions to `Availability::Ready`
  once the downloaded model is verified and resident.

## v0.5.0 - 2026-10-29

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

- **The engine now reads its own configuration from the host's `core.toml`.**
  Neither documented key was consulted before: `Engine` carried `model_setting`
  and `language` fields that nothing ever assigned, so `engines.sherpa-onnx.model`
  and `.language` were accepted by the schema, displayed in the settings UI,
  persisted across reloads, and ignored. A pinned model directory got
  auto-detection, and `language = "zh"` got `auto`. Configuration is now read at
  startup and on `ReloadConfig`, through document-level access so every host key
  outside `engines.<name>` is tolerated. A missing file, section, or key keeps
  the documented default; a wrong-typed value is an error rather than a silent
  fallback, because ignoring `model = 3` would transcribe with a model the user
  did not choose.

- **`ReloadConfig` drops the loaded recognizer**, so a changed model path or
  language takes effect instead of being silently ignored, and a read failure
  keeps the previous configuration rather than replacing it with defaults.

### Added

- **`sherpa-onnx-sys`**: a minimal FFI crate binding exactly eight functions and
  seven types from the sherpa-onnx C API. The allowlist is the contract: an
  upstream rename fails the build at a short declaration block instead of
  surfacing as an undefined symbol at runtime.

- **A `native` feature (default on)** so the pure logic — model discovery, wire
  decoding, availability policy — compiles and tests with no native library
  present. `cargo test --no-default-features` is the portable gate.

- **23 unit tests** covering model-layout resolution (direct, nested, int8
  preference, float fallback, ambiguity rejection, unrelated directories),
  wire decoding (empty, unaligned, endianness), the availability/silence
  policies, and `core.toml` parsing. All pass without sherpa-onnx installed.

- **CI split into a pure gate and a native gate.** The native gate installs a
  real model and requires `typio-engine-check` to observe a non-empty
  transcript, because the recorded defect was a green conformance run over a
  stub.

### Known limitations

- **The `setup` command is not implemented.** `Request::InvokeCommand` returns a
  bare `Ok` without doing anything, so a host asking this engine to install a
  model gets a success reply and no installation. Model acquisition is
  documented as manual: `scripts/install-sensevoice-model.sh`, or any directory
  containing `model.int8.onnx` (or `model.onnx`) plus `tokens.txt`. An earlier
  `Unreleased` section in this file advertised a background installer with
  SHA-256 verification, bounded redirects, and atomic extraction; those entries
  described the deleted C worker and were removed here because none of that code
  exists in this tree.

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
