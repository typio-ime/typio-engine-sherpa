# typio-engine-sherpa

A speech-to-text voice engine for [Typio](https://github.com/ming2k/typio),
powered by [sherpa-onnx](https://github.com/k2-fsa/sherpa-onnx). It is an
out-of-process worker: the host starts `typio-engine-sherpa` from the
`typio-engine-sherpa.toml` manifest, feeds it microphone audio, and the engine
returns recognized text.

The engine auto-detects the model type (SenseVoice, Whisper, transducer, or
Paraformer) from the files in your model directory, so the same build works
across offline sherpa-onnx models.

## Quick start

```sh
meson setup build
ninja -C build
meson install -C build
```

This installs the worker, manifest, and icons. The host discovers it on next start —
no host rebuild or registration needed. See [docs/dev/setup.md](docs/dev/setup.md)
for prerequisites, staging prefixes, and editor setup.

Then install a model so the engine has something to load:

- [How to install a speech model](docs/how-to/install-a-model.md)

## Configuration

The engine reads two keys from the host config; both are optional. See
[docs/reference/configuration.md](docs/reference/configuration.md) for the full
reference, including the model-detection rules and fixed runtime parameters.

| Key | Purpose |
| --- | --- |
| `engines.sherpa-onnx.model` | Pin a specific model subdirectory (default: auto-detect). |
| `engines.sherpa-onnx.language` | Recognition language hint (default: model-dependent). |

## License

See [LICENSE](LICENSE).
