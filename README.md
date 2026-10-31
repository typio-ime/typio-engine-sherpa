# typio-engine-sherpa

A speech-to-text voice engine for [Typio](https://github.com/ming2k/typio),
powered by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). It is an
out-of-process worker: the host starts `typio-engine-sherpa` from the
`typio-engine-sherpa.toml` manifest, feeds it microphone audio, and the engine
returns recognized text.

It recognises SenseVoice-family offline models (`model.int8.onnx` or
`model.onnx` plus `tokens.txt`). A model directory holding exactly one such
model is resolved automatically; an ambiguous base directory is rejected rather
than guessed at.

The engine is a plain Cargo build. There is no `meson`, no `libtypio` linkage,
and no in-process ABI — see Typio's ADR-0054 for why that boundary is the only
one supported.

## Quick start

```sh
cargo build --release
```

Prerequisites: `pkg-config`, `libclang` (for `bindgen`), and the sherpa-onnx C
API installed so that `pkg-config --exists sherpa-onnx` succeeds and
`<prefix>/include/sherpa-onnx/c-api/c-api.h` is present. See
[docs/dev/setup.md](docs/dev/setup.md).

You can then check the engine against Typio's black-box conformance gate:

```sh
typio-engine-check ./typio-engine-sherpa.toml
```

`cargo test --no-default-features` runs the model-discovery, wire-decoding, and
availability-policy tests with **no** native library present, so it works on any
machine.

Then install a model so the engine has something to load. You can provision
the default SenseVoice model automatically using the Typio control client:

```sh
typioctl engine sherpa-onnx:setup
```

Or install manually or via script:

- [How to install a speech model](docs/how-to/install-a-model.md)

## Configuration

The engine reads two keys from the host config; both are optional. See
[docs/reference/configuration.md](docs/reference/configuration.md) for the full
reference, including the model-detection rules and fixed runtime parameters.

| Key | Purpose |
| --- | --- |
| `engines.sherpa-onnx.model` | Pin a specific model subdirectory (default: auto-detect). |
| `engines.sherpa-onnx.language` | Recognition language hint (default: model-dependent). |
| `engines.sherpa-onnx.idle_timeout_seconds` | Model memory eviction timeout (default: 1800s / 30m; 0 disables). |

## License

See [LICENSE](LICENSE).
