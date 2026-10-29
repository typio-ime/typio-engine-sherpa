//! Minimal FFI bindings to the sherpa-onnx **offline** recognizer C API.
//!
//! Only the surface the engine actually calls is bound (see `build.rs`). This
//! crate is `unsafe` by construction: it carries raw pointers, C strings, and
//! manually managed lifetimes. Use the safe wrapper in the engine crate
//! (`sherpa.rs`) instead of calling these directly.
//!
//! The generated bindings are checked into `OUT_DIR` at build time from the
//! header that `pkg-config` resolved, so the binding and the linked library
//! always agree. Upstream ABI drift therefore fails the build rather than
//! corrupting memory at runtime.

#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
// Bindgen's output for a C API legitimately carries undocumented items; the
// crate is an FFI shim, not a public API surface.
#![allow(missing_docs)]
#![allow(clippy::all)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
