//! Safe wrapper over the sherpa-onnx offline recognizer.
//!
//! This is the only place raw sherpa pointers are touched. Everything above it
//! works with `&AudioFormat`, `&ModelFiles`, `Result`, and `Error`.
//!
//! # Ownership model
//!
//! sherpa-onnx hands back `const` pointers from `..Create..` functions and
//! requires a matching `..Destroy..` call. Each such pair is wrapped in an RAII
//! guard that owns exactly the pointer it created:
//!
//! | Create | Destroy | Guard |
//! | :--- | :--- | :--- |
//! | `SherpaOnnxCreateOfflineRecognizer` | `SherpaOnnxDestroyOfflineRecognizer` | [`Recognizer`] |
//! | `SherpaOnnxCreateOfflineStream` | `SherpaOnnxDestroyOfflineStream` | [`Stream`] |
//! | `SherpaOnnxGetOfflineStreamResult` | `SherpaOnnxDestroyOfflineRecognizerResult` | [`Result`] |
//!
//! # Threading
//!
//! sherpa's recognizer is internally synchronised for inference, but the engine
//! protocol processes one request at a time on a single thread, so this wrapper
//! deliberately does not implement `Send`/`Sync`. That keeps the "one request at
//! a time" invariant enforced by the type system rather than by convention.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::PathBuf;

use sherpa_onnx_sys as sys;

use crate::model::ModelFiles;

/// Audio geometry the recognizer expects.
pub const SAMPLE_RATE: i32 = 16_000;
/// Feature dimension the SenseVoice/Whisper families are trained with.
pub const FEATURE_DIM: i32 = 80;

/// A recognizer could not be constructed, or inference failed.
#[derive(Debug)]
pub enum Error {
    /// A path or language string contained an interior NUL byte.
    InteriorNul {
        /// The offending value.
        value: String,
    },
    /// sherpa-onnx refused to build a recognizer from the resolved files.
    RecognizerCreation {
        /// Directory the recognizer was built from.
        dir: PathBuf,
    },
    /// sherpa-onnx returned a NULL stream or result.
    NullHandle {
        /// The call that returned NULL.
        call: &'static str,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InteriorNul { value } => {
                write!(f, "value contains an interior NUL byte: {value:?}")
            }
            Self::RecognizerCreation { dir } => write!(
                f,
                "sherpa-onnx could not create a recognizer from {}",
                dir.display()
            ),
            Self::NullHandle { call } => write!(f, "{call} returned NULL"),
        }
    }
}

impl std::error::Error for Error {}

/// A `CString` owned for as long as the recognizer needs it.
///
/// sherpa-onnx stores the `const char*` fields of its config by pointer, so the
/// backing storage must outlive the recognizer, not merely the create call.
struct CStrings(Vec<CString>);

impl CStrings {
    fn push(&mut self, value: &str) -> Result<*const c_char, Error> {
        let owned = CString::new(value).map_err(|_| Error::InteriorNul {
            value: value.to_string(),
        })?;
        self.0.push(owned);
        Ok(self.0.last().expect("just pushed").as_ptr())
    }
}

/// An offline recognizer bound to one model directory.
pub struct Recognizer {
    raw: *const sys::SherpaOnnxOfflineRecognizer,
    // Config fields are borrowed by sherpa for the recognizer's lifetime, so the
    // backing C strings must outlive the pointer above.
    _owned: CStrings,
}

impl Recognizer {
    /// Build a recognizer from resolved model files.
    ///
    /// # Errors
    /// Returns an error when a path cannot be converted to a C string, or when
    /// sherpa-onnx declines to construct the recognizer (typically a corrupt or
    /// mismatched model file).
    pub fn new(files: &ModelFiles, language: &str) -> Result<Self, Error> {
        let mut owned = CStrings(Vec::new());
        let model = owned.push(&files.model_path().to_string_lossy())?;
        let tokens = owned.push(&files.tokens_path().to_string_lossy())?;
        let provider = owned.push("cpu")?;
        let decoding = owned.push("greedy_search")?;
        let language = owned.push(language)?;

        // sherpa-onnx requires a fully zeroed config; unspecified families stay
        // null, which is how the library detects the model type.
        let mut config: sys::SherpaOnnxOfflineRecognizerConfig = unsafe { std::mem::zeroed() };
        config.feat_config = sys::SherpaOnnxFeatureConfig {
            sample_rate: SAMPLE_RATE,
            feature_dim: FEATURE_DIM,
        };
        config.model_config.tokens = tokens;
        config.model_config.num_threads = 2;
        config.model_config.debug = 0;
        config.model_config.provider = provider;
        config.model_config.sense_voice = sys::SherpaOnnxOfflineSenseVoiceModelConfig {
            model,
            language,
            // Inverse text normalisation turns "一千二" into "1200", which is
            // what a user dictating numbers expects.
            use_itn: 1,
        };
        config.decoding_method = decoding;
        config.max_active_paths = 4;

        let raw = unsafe { sys::SherpaOnnxCreateOfflineRecognizer(&config) };
        if raw.is_null() {
            return Err(Error::RecognizerCreation {
                dir: files.dir().to_path_buf(),
            });
        }
        Ok(Self { raw, _owned: owned })
    }

    /// Transcribe mono 16 kHz f32 samples.
    ///
    /// # Errors
    /// Returns [`Error::NullHandle`] when sherpa-onnx fails to allocate a stream
    /// or to return a result.
    pub fn transcribe(&self, samples: &[f32]) -> Result<String, Error> {
        if samples.is_empty() {
            return Ok(String::new());
        }
        let stream = Stream::new(self.raw)?;
        unsafe {
            sys::SherpaOnnxAcceptWaveformOffline(
                stream.raw,
                SAMPLE_RATE,
                samples.as_ptr(),
                samples.len() as i32,
            );
            sys::SherpaOnnxDecodeOfflineStream(self.raw, stream.raw);
        }
        let result = ResultGuard::new(stream.raw)?;
        // SAFETY: `text` is a NUL-terminated C string owned by the result, which
        // the guard keeps alive for this borrow.
        let text = unsafe { CStr::from_ptr((*result.raw).text) }
            .to_string_lossy()
            .into_owned();
        Ok(text.trim().to_string())
    }
}

impl Drop for Recognizer {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { sys::SherpaOnnxDestroyOfflineRecognizer(self.raw) };
        }
    }
}

/// RAII owner of one decode stream.
struct Stream {
    raw: *const sys::SherpaOnnxOfflineStream,
}

impl Stream {
    fn new(recognizer: *const sys::SherpaOnnxOfflineRecognizer) -> Result<Self, Error> {
        let raw = unsafe { sys::SherpaOnnxCreateOfflineStream(recognizer) };
        if raw.is_null() {
            return Err(Error::NullHandle {
                call: "SherpaOnnxCreateOfflineStream",
            });
        }
        Ok(Self { raw })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { sys::SherpaOnnxDestroyOfflineStream(self.raw) };
        }
    }
}

/// RAII owner of one recognition result.
struct ResultGuard {
    raw: *const sys::SherpaOnnxOfflineRecognizerResult,
}

impl ResultGuard {
    fn new(stream: *const sys::SherpaOnnxOfflineStream) -> Result<Self, Error> {
        let raw = unsafe { sys::SherpaOnnxGetOfflineStreamResult(stream) };
        if raw.is_null() {
            return Err(Error::NullHandle {
                call: "SherpaOnnxGetOfflineStreamResult",
            });
        }
        Ok(Self { raw })
    }
}

impl Drop for ResultGuard {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { sys::SherpaOnnxDestroyOfflineRecognizerResult(self.raw) };
        }
    }
}
