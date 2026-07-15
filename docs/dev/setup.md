# Development Setup

## Prerequisites

| Dependency        | Provides                    | Notes                           |
| ----------------- | --------------------------- | ------------------------------- |
| Meson `>= 1.0.0`  | Build system                | Requires `c_std=c23`.           |
| Ninja             | Build backend               |                                 |
| C23 compiler      | Compilation                 | GCC 13+ or Clang 16+.           |
| `typio-engine-abi` | Engine ABI headers + pc    | From the Typio workspace.      |
| `sherpa-onnx`     | STT runtime                 | Falls back to `LIBRARY_PATH`.   |

## Build

```sh
meson setup build
ninja -C build
```

## Development Run

With a release build of the Typio workspace:

```sh
LD_LIBRARY_PATH=/path/to/typio/target/release \
  /path/to/typio/target/release/typio --verbose \
  --engine-dir "$PWD/build"
```

The build directory contains `typio-engine-sherpa` and its development
manifest. The manifest starts `./typio-engine-sherpa` relative to that
directory.

<details>
<summary>Using a local libtypio build</summary>

Adjust `/path/to/typio` for your checkout.

```sh
cargo build --release -p typio-core --manifest-path /path/to/typio/Cargo.toml
export PKG_CONFIG_PATH="/path/to/typio/target/release${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
meson setup --wipe build
ninja -C build
```

</details>

## Rebuild

```sh
ninja -C build
```

Reconfigure when changing build options or dependency layout:

```sh
meson setup --reconfigure build
```

See [How to Install a Speech Model](../how-to/install-a-model.md) for runtime
model setup. Production installation belongs in distribution packaging.

## Models

The worker discovers a model at focus-in under
`<instance-data-dir>/sherpa-onnx/`. Place a single model directory there; type
(SenseVoice, Whisper, transducer, Paraformer) is auto-detected from the files
present. Override with `engines.sherpa-onnx.model` and
`engines.sherpa-onnx.language`.

## Editor / LSP

```sh
ln -sf build/compile_commands.json compile_commands.json
```
