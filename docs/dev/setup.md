# Development setup

## Prerequisites

| Requirement | Why |
| :--- | :--- |
| Rust 1.85+ (edition 2024) | Builds the engine. |
| `pkg-config` | Resolves the sherpa-onnx C API. |
| `libclang` | `bindgen` generates the FFI bindings from the sherpa-onnx header. |
| sherpa-onnx C API | Provides `sherpa-onnx.pc` and `sherpa-onnx/c-api/c-api.h`. |

Verify the native prerequisites:

```sh
pkg-config --exists sherpa-onnx && echo ok
pkg-config --modversion sherpa-onnx
```

## Build

```sh
cargo build --release
```

This produces `target/release/typio-engine-sherpa`, the path the manifest names.

## Test

Two configurations, both meaningful:

```sh
# Pure logic: model discovery, wire decoding, availability policy.
# Needs no sherpa-onnx and no libclang. This is the portable gate.
cargo test --no-default-features

# The shipped configuration. Links sherpa-onnx; requires it at runtime.
cargo test
```

`--no-default-features` is the only configuration that runs on a machine without
the native library, which is why the pure logic lives in `src/model.rs` with no
FFI at all.

## Conformance

Typio's engine gate is black-box and lives in the host repository:

```sh
typio-engine-check ./typio-engine-sherpa.toml
```

It starts the engine in an isolated process, performs the handshake, and — for an
engine that declares `voice_input` — sends real non-silent audio and requires a
non-empty transcript. A stub that returns empty text fails.

## sherpa-onnx from source

If your distribution ships no sherpa-onnx package, build the C API from a pinned
revision and point the sys crate at it:

```sh
git clone https://github.com/k2-fsa/sherpa-onnx
cd sherpa-onnx
git checkout 7fc2ea7725425d2df7516fdc17e6abe6bd9518d7
cmake -S . -B build -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX="$HOME/.local" \
  -DBUILD_SHARED_LIBS=ON -DSHERPA_ONNX_ENABLE_C_API=ON
cmake --build build --target install
```

Then export `PKG_CONFIG_PATH="$HOME/.local/lib/pkgconfig"` and, if the prefix is
outside the loader's default search path, `LD_LIBRARY_PATH="$HOME/.local/lib"`.
The sys crate bakes an rpath for non-system prefixes, so a normal prefix needs
neither.

## Overriding discovery

The `sherpa-onnx-sys` build script honours these when `pkg-config` is not usable:

| Variable | Effect |
| :--- | :--- |
| `SHERPA_ONNX_SYS_USE_PKG_CONFIG=0` | Skip the probe; link manually. |
| `SHERPA_ONNX_SYS_INCLUDE_DIR` | Colon-separated extra include directories. |
| `SHERPA_ONNX_SYS_LIB_DIR` | Colon-separated extra library directories. |
| `SHERPA_ONNX_SYS_STATIC=1` | Link statically. |

## Model

Install a model, then confirm the layout the engine expects:

```sh
~/.local/share/typio/sherpa-onnx/<model-name>/
    model.int8.onnx
    tokens.txt
```

Use [How to install a speech model](../how-to/install-a-model.md).
