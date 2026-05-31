# Development Setup

## Prerequisites

| Dependency        | Provides                    | Notes                           |
| ----------------- | --------------------------- | ------------------------------- |
| Meson `>= 1.0.0`  | Build system                | Requires `c_std=c23`.           |
| Ninja             | Build backend               |                                 |
| C23 compiler      | Compilation                 | GCC 13+ or Clang 16+.           |
| `typio-engine-abi`| Engine ABI headers + pc     | From libtypio.                  |
| `sherpa-onnx`     | STT runtime                 | Falls back to `LIBRARY_PATH`.   |

## Build & Install

```sh
meson setup build --prefix "$HOME/.local"
ninja -C build
meson install -C build
```

Plugin installs to `~/.local/lib/typio/engines/typio_engine_sherpa.so`.

<details>
<summary>Using a local libtypio build</summary>

Adjust the paths if your checkout is not at `../libtypio`.

```sh
cargo build --manifest-path ../libtypio/Cargo.toml
export PKG_CONFIG_PATH="../libtypio/target/debug${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
meson setup --wipe build --prefix "$HOME/.local"
ninja -C build
meson install -C build
```

</details>

## Rebuild

```sh
ninja -C build && meson install -C build
```

Reconfigure when changing build options or dependency layout:

```sh
meson setup --reconfigure build --prefix "$HOME/.local"
```

## Models

The plugin discovers a model at focus-in under
`<instance-data-dir>/sherpa-onnx/`. Place a single model directory there; type
(SenseVoice, Whisper, transducer, Paraformer) is auto-detected from the files
present. Override with `engines.sherpa-onnx.model` and
`engines.sherpa-onnx.language`.

## Editor / LSP

```sh
ln -sf build/compile_commands.json compile_commands.json
```
