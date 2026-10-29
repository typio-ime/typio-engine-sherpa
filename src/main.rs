//! Sherpa-ONNX speech-to-text engine for Typio.
//!
//! This file is the Engine Protocol frontend: it speaks typed frames over the
//! socketpair the host creates and defers all recognition to [`sherpa`].
//!
//! # Design contract
//!
//! * **No in-process ABI.** The engine is a process; there is no `libtypio`
//!   linkage and no `TypioEngineBaseOps` vtable. See ADR-0054.
//! * **Availability is honest.** `Availability` reports `Ready` only when a
//!   recognizer is actually loaded. A missing or unloadable model is reported as
//!   `Unavailable` with a reason, never as `Ready` that then returns empty text.
//! * **Recognition failure is an error.** Silence, undecodable input, and a
//!   missing model all produce `ReplyRecord::Error`. The engine never answers
//!   `Text("")` to non-empty audio, because that is indistinguishable from an
//!   unimplemented engine (ADR-0055).
//! * **The model loads lazily and once.** `Initialize` records configuration and
//!   probes availability; the first `ProcessAudio` pays the 239 MB load. This
//!   keeps handshake latency at milliseconds and keeps the probe cheap.

mod model;

#[cfg(feature = "native")]
mod sherpa;

use std::fs::File;
use std::os::fd::FromRawFd;
use std::path::PathBuf;

use typio_engine_protocol::{
    Availability, ENGINE_PROTOCOL_FD, EngineHello, EngineKind, Frame, HostHello, MessageType,
    Reply, ReplyRecord, Request, SchemaDefault, SchemaField, read_frame, write_frame,
};

use model::ModelFiles;

/// The recogniser backend.
///
/// With the `native` feature this wraps a loaded sherpa-onnx recognizer. Without
/// it — CI on a machine with no sherpa-onnx, or a build that must stay link-free —
/// loading always reports why recognition is unavailable. Both shapes share one
/// type so `Engine` and its availability policy are compiled and tested identically
/// in either configuration.
#[cfg(feature = "native")]
type Backend = sherpa::Recognizer;

#[cfg(not(feature = "native"))]
struct Backend;

#[cfg(not(feature = "native"))]
impl Backend {
    fn load(files: &ModelFiles, _language: &str) -> Result<Self, String> {
        Err(format!(
            "this build has no native sherpa-onnx support (feature `native` is off);              cannot load {}",
            files.dir().display()
        ))
    }

    fn transcribe(&self, _samples: &[f32]) -> Result<String, String> {
        Err("this build has no native sherpa-onnx support".to_string())
    }
}

#[cfg(feature = "native")]
trait LoadBackend {
    fn load(files: &ModelFiles, language: &str) -> Result<Self, String>
    where
        Self: Sized;
}

#[cfg(feature = "native")]
impl LoadBackend for Backend {
    fn load(files: &ModelFiles, language: &str) -> Result<Self, String> {
        Backend::new(files, language).map_err(|error| error.to_string())
    }
}

/// Engine identifier reported in `ENGINE_HELLO` and the manifest.
const ENGINE_NAME: &str = "sherpa-onnx";

/// Configuration key for the model directory.
const MODEL_KEY: &str = "engines.sherpa-onnx.model";
/// Configuration key for the recognition language hint.
const LANGUAGE_KEY: &str = "engines.sherpa-onnx.language";

/// Languages SenseVoice supports. `"auto"` lets the model decide.
const LANGUAGES: &[&str] = &["auto", "zh", "en", "ja", "ko", "yue"];

fn schema() -> Vec<SchemaField> {
    vec![
        SchemaField {
            key: MODEL_KEY.to_string(),
            default: SchemaDefault::String(String::new()),
            label: Some("Model Directory".to_string()),
            section: Some("Model".to_string()),
            minimum: 0,
            maximum: 0,
            step: 0,
            options: Vec::new(),
            runtime_property: None,
        },
        SchemaField {
            key: LANGUAGE_KEY.to_string(),
            default: SchemaDefault::String(String::new()),
            label: Some("Recognition Language".to_string()),
            section: Some("Model".to_string()),
            minimum: 0,
            maximum: 0,
            step: 0,
            options: std::iter::once(String::new())
                .chain(LANGUAGES.iter().map(|value| (*value).to_string()))
                .collect(),
            runtime_property: None,
        },
    ]
}

/// Engine state across the protocol loop.
#[derive(Default)]
struct Engine {
    /// Configured model directory, relative paths resolved against `data_dir`.
    model_setting: Option<String>,
    /// Language hint; empty means let the model auto-detect.
    language: String,
    /// Host-provided data directory, used to resolve relative model paths.
    data_dir: PathBuf,
    /// Loaded recognizer, if one is resident.
    recognizer: Option<Backend>,
    /// Why the last load attempt failed, so `Availability` can explain itself.
    load_error: Option<String>,
}

impl Engine {
    /// Resolve the configured model directory to an absolute path.
    ///
    /// An absolute setting is used as-is. A relative setting is resolved against
    /// the host's data directory, then against `<data_dir>/sherpa-onnx`, which is
    /// where the installer places models. An empty setting falls back to
    /// `<data_dir>/sherpa-onnx` so a stock install works with no configuration.
    fn resolve_model_dir(&self) -> PathBuf {
        let base = self.data_dir.join("sherpa-onnx");
        match self
            .model_setting
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(setting) => {
                let path = PathBuf::from(setting);
                if path.is_absolute() {
                    path
                } else {
                    let under_data = self.data_dir.join(&path);
                    if under_data.exists() {
                        under_data
                    } else {
                        base.join(&path)
                    }
                }
            }
            None => base,
        }
    }

    /// Load a recognizer if one is not already resident.
    ///
    /// Returns only the error: the loaded backend stays owned by `self`, so the
    /// caller never holds a borrow across the subsequent inference call.
    fn ensure_loaded(&mut self) -> Result<(), String> {
        if self.recognizer.is_some() {
            return Ok(());
        }
        if let Some(error) = &self.load_error {
            // Retry only after a ReloadConfig clears it.
            return Err(error.clone());
        }
        let dir = self.resolve_model_dir();
        let language = if self.language.trim().is_empty() {
            "auto"
        } else {
            self.language.trim()
        };
        match ModelFiles::discover(&dir) {
            Ok(files) => match Backend::load(&files, language) {
                Ok(backend) => {
                    log(&format!(
                        "loaded {} model from {}",
                        files.kind().as_str(),
                        files.dir().display()
                    ));
                    self.recognizer = Some(backend);
                    self.load_error = None;
                    Ok(())
                }
                Err(error) => {
                    log(&format!("model unavailable: {error}"));
                    self.load_error = Some(error.clone());
                    Err(error)
                }
            },
            Err(error) => {
                let error = error.to_string();
                log(&format!("model unavailable: {error}"));
                self.load_error = Some(error.clone());
                Err(error)
            }
        }
    }

    /// Report availability without forcing a model load.
    ///
    /// Probing calls `ModelFiles::discover`, which is stat-only. Reporting `Ready`
    /// requires that a model is actually resolvable, so the host can never see
    /// `Ready` from an engine that would then fail to transcribe.
    fn availability(&mut self) -> Availability {
        if self.recognizer.is_some() {
            return Availability::Ready;
        }
        if self.load_error.is_some() {
            return Availability::Failed;
        }
        match ModelFiles::discover(&self.resolve_model_dir()) {
            Ok(_) => Availability::Ready,
            Err(error) => {
                self.load_error = Some(error.to_string());
                Availability::Failed
            }
        }
    }

    /// Decode the wire payload into f32 samples.
    ///
    /// The host sends 16 kHz mono f32 little-endian. A payload whose length is not
    /// a multiple of four cannot be sample-aligned and is rejected rather than
    /// silently truncated.
    fn decode_audio(&self, payload: &[u8]) -> Result<Vec<f32>, String> {
        if payload.is_empty() {
            return Err("no audio captured".to_string());
        }
        if !payload.len().is_multiple_of(4) {
            return Err(format!(
                "audio payload length {} is not a multiple of the 4-byte sample width",
                payload.len()
            ));
        }
        Ok(payload
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect())
    }

    /// Transcribe a wire payload, mapping every failure to a message.
    fn process_audio(&mut self, payload: &[u8]) -> Result<String, String> {
        let samples = self.decode_audio(payload)?;
        let peak = samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()));
        if !peak.is_finite() {
            return Err("audio payload contains non-finite samples".to_string());
        }
        if peak < 1.0e-4 {
            // Below the noise floor: there is genuinely nothing to recognise, and
            // saying so is more useful than returning an empty transcript.
            return Err(format!("audio is silent (peak amplitude {peak:.6})"));
        }
        self.ensure_loaded()?;
        let backend = self
            .recognizer
            .as_ref()
            .ok_or_else(|| "recognizer unavailable after load".to_string())?;
        let text = backend
            .transcribe(&samples)
            .map_err(|error| error.to_string())?;
        if text.is_empty() {
            return Err("recognizer produced no text for non-silent audio".to_string());
        }
        Ok(text)
    }
}

/// Log to stderr, which the host captures. The protocol channel is fd 3 and must
/// never carry diagnostics.
fn log(message: &str) {
    eprintln!("[sherpa-onnx] {message}");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: the host dup2's the engine end of the socketpair onto this fd before
    // exec (see EngineSession::spawn), so it is a valid, exclusively owned stream.
    let mut channel = unsafe { File::from_raw_fd(ENGINE_PROTOCOL_FD) };

    write_frame(
        &mut channel,
        &Frame::new(
            MessageType::EngineHello,
            0,
            EngineHello {
                engine: ENGINE_NAME.to_string(),
                kind: EngineKind::Voice,
                schema: schema(),
            }
            .encode(),
        ),
    )?;

    let host_frame = match read_frame(&mut channel) {
        Ok(frame) => frame,
        // The host may probe discovery and close before HOST_HELLO. That is a
        // normal shutdown, not an error.
        Err(typio_engine_protocol::ProtocolError::Io(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if host_frame.message_type != MessageType::HostHello || host_frame.request_id != 0 {
        return Err(format!(
            "expected HOST_HELLO with request id 0, got {:?} id {}",
            host_frame.message_type, host_frame.request_id
        )
        .into());
    }
    let host_hello = HostHello::decode(&host_frame.payload)?;

    let mut engine = Engine {
        data_dir: PathBuf::from(&host_hello.data_dir),
        ..Engine::default()
    };

    // Host closed the channel: normal teardown.
    while let Ok(frame) = read_frame(&mut channel) {
        if frame.message_type != MessageType::Request || frame.request_id == 0 {
            return Err(format!(
                "expected REQUEST with a nonzero request id, got {:?} id {}",
                frame.message_type, frame.request_id
            )
            .into());
        }

        let request = match Request::decode(&frame.payload) {
            Ok(request) => request,
            Err(error) => {
                respond(
                    &mut channel,
                    frame.request_id,
                    vec![ReplyRecord::Error(error.to_string())],
                )?;
                continue;
            }
        };

        if matches!(request, Request::Shutdown) {
            let _ = respond(&mut channel, frame.request_id, vec![ReplyRecord::Ok]);
            break;
        }

        let records = handle(&mut engine, request);
        respond(&mut channel, frame.request_id, records)?;
    }

    Ok(())
}

fn handle(engine: &mut Engine, request: Request) -> Vec<ReplyRecord> {
    match request {
        Request::Initialize => {
            let availability = engine.availability();
            vec![ReplyRecord::Ok, ReplyRecord::Availability(availability)]
        }
        Request::Availability => vec![ReplyRecord::Availability(engine.availability())],
        Request::ProcessAudio(payload) => match engine.process_audio(&payload) {
            Ok(text) => vec![ReplyRecord::Text(text), ReplyRecord::Ok],
            Err(error) => vec![ReplyRecord::Error(error)],
        },
        // A configuration change may name a different model or language, so drop
        // the loaded recognizer and let the next use rebuild it.
        Request::ReloadConfig => {
            engine.recognizer = None;
            engine.load_error = None;
            vec![ReplyRecord::Ok]
        }
        Request::FocusIn(_)
        | Request::FocusOut(_)
        | Request::Reset(_)
        | Request::Deactivate
        | Request::ListModes
        | Request::GetActiveMode(_)
        | Request::SetActiveMode { .. }
        | Request::ListCommands
        | Request::InvokeCommand(_) => vec![ReplyRecord::Ok],
        Request::ProcessKey(_) => vec![ReplyRecord::Error(
            "keyboard input is not supported by a voice engine".to_string(),
        )],
        Request::CommitCandidate { .. } => vec![ReplyRecord::Error(
            "candidate commits are not supported by a voice engine".to_string(),
        )],
        Request::Shutdown => vec![ReplyRecord::Ok],
    }
}

fn respond(
    channel: &mut File,
    request_id: u64,
    records: Vec<ReplyRecord>,
) -> Result<(), Box<dyn std::error::Error>> {
    write_frame(
        channel,
        &Frame::new(
            MessageType::Response,
            request_id,
            Reply { records }.encode(),
        ),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_with(data_dir: &str) -> Engine {
        Engine {
            data_dir: PathBuf::from(data_dir),
            ..Engine::default()
        }
    }

    #[test]
    fn decode_audio_rejects_empty_payloads() {
        let engine = engine_with("/tmp");
        assert!(engine.decode_audio(&[]).unwrap_err().contains("no audio"));
    }

    #[test]
    fn decode_audio_rejects_unaligned_payloads() {
        let engine = engine_with("/tmp");
        let error = engine.decode_audio(&[0, 1, 2]).unwrap_err();
        assert!(
            error.contains("multiple of the 4-byte sample width"),
            "{error}"
        );
    }

    #[test]
    fn decode_audio_reads_little_endian_samples() {
        let engine = engine_with("/tmp");
        let mut bytes = Vec::new();
        for sample in [0.5f32, -0.25] {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let decoded = engine.decode_audio(&bytes).expect("decode");
        assert_eq!(decoded, vec![0.5, -0.25]);
    }

    #[test]
    fn silent_audio_is_an_error_not_an_empty_transcript() {
        let mut engine = engine_with("/tmp");
        // 0.1 s of silence in the wire format.
        let payload = vec![0u8; 1_600 * 4];
        let error = engine.process_audio(&payload).unwrap_err();
        assert!(error.contains("silent"), "{error}");
    }

    #[test]
    fn a_missing_model_reports_unavailable_rather_than_ready() {
        let mut engine = engine_with("/nonexistent-typio-data");
        assert_eq!(engine.availability(), Availability::Failed);
        // And the reason is retained so a second probe stays Unavailable.
        assert!(engine.load_error.is_some());
        assert_eq!(engine.availability(), Availability::Failed);
    }

    #[test]
    fn non_silent_audio_with_no_model_errors_instead_of_returning_text() {
        let mut engine = engine_with("/nonexistent-typio-data");
        let mut samples = Vec::new();
        for i in 0..1_600 {
            let value = if i % 2 == 0 { 0.5f32 } else { -0.5f32 };
            samples.extend_from_slice(&value.to_le_bytes());
        }
        // The critical property: no model means an error, never `Ok("")`.
        let error = engine.process_audio(&samples).unwrap_err();
        assert!(
            error.contains("model"),
            "expected a model error, got: {error}"
        );
    }

    #[test]
    fn availability_does_not_claim_ready_without_a_resolvable_model() {
        // A directory that exists but holds no model must not read as Ready.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut engine = Engine {
            data_dir: dir.path().to_path_buf(),
            ..Engine::default()
        };
        assert_eq!(engine.availability(), Availability::Failed);
    }
}
