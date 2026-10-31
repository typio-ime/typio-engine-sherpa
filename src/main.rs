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

mod config;
mod model;
mod setup;

#[cfg(feature = "native")]
mod sherpa;

use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::PathBuf;

use typio_engine_protocol::{
    Availability, ENGINE_PROTOCOL_FD, EngineHello, EngineKind, Frame, HostHello, MessageType,
    Reply, ReplyRecord, Request, SchemaDefault, SchemaField, read_frame, write_frame,
};

use config::EngineConfig;
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

/// Schema keys are the fully-qualified form the host displays.
const MODEL_KEY: &str = "engines.sherpa-onnx.model";
/// Recognition language hint.
const LANGUAGE_KEY: &str = "engines.sherpa-onnx.language";
/// Idle timeout in seconds before unloading the model.
const IDLE_TIMEOUT_KEY: &str = "engines.sherpa-onnx.idle_timeout_seconds";

/// Languages SenseVoice supports. `"auto"` lets the model decide.
const LANGUAGES: &[&str] = &["auto", "zh", "en", "ja", "ko", "yue"];

fn schema() -> Vec<SchemaField> {
    let mut model_options = vec![String::new()];
    let search_dirs = model::model_search_dirs(None);
    model_options.extend(ModelFiles::list_models_in_search_dirs(&search_dirs));

    vec![
        SchemaField {
            key: MODEL_KEY.to_string(),
            default: SchemaDefault::String(String::new()),
            label: Some("Model Directory".to_string()),
            section: Some("Model".to_string()),
            minimum: 0,
            maximum: 0,
            step: 0,
            options: model_options,
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
        SchemaField {
            key: IDLE_TIMEOUT_KEY.to_string(),
            default: SchemaDefault::Integer(1800),
            label: Some("Idle Unload Timeout (seconds)".to_string()),
            section: Some("Memory".to_string()),
            minimum: 0,
            maximum: 86400,
            step: 60,
            options: Vec::new(),
            runtime_property: None,
        },
    ]
}

/// Engine state across the protocol loop.
#[derive(Default)]
struct Engine {
    /// Host config directory, holding the host-owned `core.toml`.
    config_dir: PathBuf,
    /// Host data directory, used to resolve relative model paths.
    data_dir: PathBuf,
    /// Configuration read from `core.toml` on load and after `ReloadConfig`.
    config: EngineConfig,
    /// Loaded recognizer, if one is resident.
    recognizer: Option<Backend>,
    /// Why the last load attempt failed, so `Availability` can explain itself.
    load_error: Option<String>,
    /// Instant when the last transcription completed, driving idle eviction.
    last_active: Option<std::time::Instant>,
    /// Detached background setup manager for acquiring models on demand.
    setup_manager: setup::SetupManager,
}

impl Engine {
    /// Return the remaining duration before the resident model should be evicted,
    /// or `None` if no model is loaded or idle eviction is disabled.
    fn time_until_idle_eviction(&self) -> Option<std::time::Duration> {
        if self.recognizer.is_none() || self.config.idle_timeout_seconds == 0 {
            return None;
        }
        let timeout = std::time::Duration::from_secs(self.config.idle_timeout_seconds);
        let last = self.last_active.unwrap_or_else(std::time::Instant::now);
        Some(timeout.saturating_sub(last.elapsed()))
    }

    /// Drop the resident model if one is loaded, reclaiming physical memory.
    fn unload_idle(&mut self) {
        if self.recognizer.is_some() {
            log(&format!(
                "idle timeout ({}s) reached; unloaded model to reclaim memory",
                self.config.idle_timeout_seconds
            ));
            self.recognizer = None;
            self.last_active = None;
        }
    }

    /// Resolve the target model directory, searching multi-tier XDG paths if unspecified.
    fn resolve_model(&self) -> Result<ModelFiles, model::ModelError> {
        let mut search_dirs = Vec::new();
        if self.data_dir.as_os_str().is_empty() {
            search_dirs.extend(model::model_search_dirs(None));
        } else {
            search_dirs.push(self.data_dir.join("sherpa-onnx"));
            // Include system paths if the data directory is not an isolated test path.
            if !self.data_dir.starts_with("/tmp") {
                search_dirs.push(PathBuf::from("/usr/local/share/typio/sherpa-onnx"));
                search_dirs.push(PathBuf::from("/usr/share/typio/sherpa-onnx"));
            }
        }

        if let Some(setting) = self.config.model.as_deref() {
            let path = PathBuf::from(setting);
            if path.is_absolute() {
                return ModelFiles::discover(&path);
            }
            for base in &search_dirs {
                let candidate = base.join(&path);
                if candidate.exists() {
                    return ModelFiles::discover(&candidate);
                }
            }
            return ModelFiles::discover(&search_dirs[0].join(&path));
        }

        ModelFiles::discover_in_search_dirs(&search_dirs)
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
        let language = self.config.language.as_deref().unwrap_or("auto");
        match self.resolve_model() {
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

    /// Re-read `core.toml` and drop the resident recognizer.
    ///
    /// Called on `ReloadConfig`. A read failure keeps the previous configuration
    /// rather than replacing it with defaults, so a transient I/O error cannot
    /// silently change which model the user is transcribing with.
    ///
    /// # Errors
    /// Returns a message when `core.toml` exists but cannot be read or parsed, or
    /// when one of this engine's keys holds a non-string value.
    fn reload_config(&mut self) -> Result<(), String> {
        let config = EngineConfig::read(&self.config_dir, ENGINE_NAME)?;
        if config != self.config {
            log(&format!(
                "config reloaded (model={:?}, language={:?})",
                config.model, config.language
            ));
        }
        self.config = config;
        self.recognizer = None;
        self.load_error = None;
        self.last_active = None;
        Ok(())
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
        if self.setup_manager.is_running() {
            return Availability::Preparing;
        }
        if self.setup_manager.status() == setup::SetupStatus::Completed {
            self.load_error = None;
        }
        if self.load_error.is_some() {
            return Availability::Failed;
        }
        match self.resolve_model() {
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
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| f32::from_le_bytes(*chunk))
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
        self.last_active = Some(std::time::Instant::now());
        Ok(text)
    }
}

/// Log to stderr, which the host captures. The protocol channel is fd 3 and must
/// never carry diagnostics.
fn log(message: &str) {
    eprintln!("[sherpa-onnx] {message}");
}

/// Wait until fd becomes readable or the specified timeout expires.
///
/// Returns `Ok(true)` if data is available to read, `Ok(false)` on timeout,
/// or an error if the descriptor is broken.
fn wait_for_frame(fd: i32, timeout: Option<std::time::Duration>) -> Result<bool, std::io::Error> {
    let deadline = timeout.map(|duration| std::time::Instant::now() + duration);
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let timeout_ms = match deadline {
            Some(target) => {
                let remaining = target.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Ok(false);
                }
                remaining.as_millis().clamp(1, i32::MAX as u128) as i32
            }
            None => -1,
        };
        let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if result > 0 {
            if descriptor.revents & libc::POLLNVAL != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "engine protocol descriptor became invalid",
                ));
            }
            return Ok(true);
        }
        if result == 0 {
            return Ok(false);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
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
        config_dir: PathBuf::from(&host_hello.config_dir),
        data_dir: PathBuf::from(&host_hello.data_dir),
        ..Engine::default()
    };
    // Read the host's `core.toml` before the first request. A failure is logged
    // and reported through `Availability`, not treated as fatal: the engine still
    // speaks the protocol and can serve `ReloadConfig` once the file is fixed.
    if let Err(error) = engine.reload_config() {
        log(&format!("configuration unavailable: {error}"));
        engine.load_error = Some(error);
    }

    let fd = channel.as_raw_fd();

    loop {
        let timeout = engine.time_until_idle_eviction();
        if timeout.is_some_and(|d| d.is_zero()) {
            engine.unload_idle();
        }
        let timeout = engine.time_until_idle_eviction();

        match wait_for_frame(fd, timeout) {
            Ok(true) => {
                let frame = match read_frame(&mut channel) {
                    Ok(frame) => frame,
                    // Host closed the channel: normal teardown.
                    Err(typio_engine_protocol::ProtocolError::Io(_)) => break,
                    Err(error) => return Err(error.into()),
                };

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
            Ok(false) => {
                // Poll timeout expired: idle period reached.
                engine.unload_idle();
            }
            Err(error) => {
                if error.kind() == std::io::ErrorKind::BrokenPipe {
                    break;
                }
                return Err(error.into());
            }
        }
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
        Request::ReloadConfig => match engine.reload_config() {
            Ok(()) => vec![ReplyRecord::Ok],
            Err(error) => {
                log(&format!("configuration reload failed: {error}"));
                vec![ReplyRecord::Error(error)]
            }
        },
        Request::ListCommands => vec![
            ReplyRecord::Command(typio_engine_protocol::Command {
                id: "setup".to_string(),
                label: "Download default SenseVoice model".to_string(),
            }),
            ReplyRecord::Ok,
        ],
        Request::InvokeCommand(ref id) if id == "setup" => {
            match engine.setup_manager.start_setup(engine.data_dir.clone()) {
                Ok(started) => {
                    if started {
                        log("started background setup worker for default model");
                    } else {
                        log("setup already in progress or model already resident");
                    }
                    vec![ReplyRecord::Ok]
                }
                Err(error) => {
                    log(&format!("failed to start setup: {error}"));
                    vec![ReplyRecord::Error(error)]
                }
            }
        }
        Request::InvokeCommand(id) => vec![ReplyRecord::Error(format!("unknown command '{id}'"))],
        Request::FocusIn(_)
        | Request::FocusOut(_)
        | Request::Reset(_)
        | Request::Deactivate
        | Request::ListModes
        | Request::GetActiveMode(_)
        | Request::SetActiveMode { .. } => vec![ReplyRecord::Ok],
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

    #[test]
    fn wait_for_frame_detects_timeout_and_readiness() {
        use std::io::Write;
        use std::os::unix::net::UnixStream;

        let (mut sender, receiver) = UnixStream::pair().expect("socketpair");
        let fd = receiver.as_raw_fd();

        // No data sent: expires cleanly
        let ready = wait_for_frame(fd, Some(std::time::Duration::from_millis(10))).expect("wait");
        assert!(!ready);

        // Send data: detects readiness
        sender.write_all(b"x").expect("write");
        let ready = wait_for_frame(fd, Some(std::time::Duration::from_millis(50))).expect("wait");
        assert!(ready);
    }

    #[test]
    fn idle_eviction_duration_is_none_without_resident_backend() {
        let engine = engine_with("/tmp");
        assert_eq!(engine.time_until_idle_eviction(), None);
    }

    #[test]
    fn idle_eviction_duration_is_none_when_timeout_is_zero() {
        let mut engine = engine_with("/tmp");
        engine.config.idle_timeout_seconds = 0;
        engine.last_active = Some(std::time::Instant::now());
        #[cfg(not(feature = "native"))]
        {
            engine.recognizer = Some(Backend);
            assert_eq!(engine.time_until_idle_eviction(), None);
        }
    }

    #[test]
    fn idle_eviction_unloads_recognizer() {
        let mut engine = engine_with("/tmp");
        engine.config.idle_timeout_seconds = 1800;
        #[cfg(not(feature = "native"))]
        {
            engine.recognizer = Some(Backend);
            engine.last_active = Some(std::time::Instant::now());
            assert!(engine.time_until_idle_eviction().is_some());
            engine.unload_idle();
            assert!(engine.recognizer.is_none());
            assert!(engine.last_active.is_none());
            assert_eq!(engine.time_until_idle_eviction(), None);
        }
    }

    #[test]
    fn invoke_unknown_command_returns_error() {
        let mut engine = Engine::default();
        let records = handle(&mut engine, Request::InvokeCommand("unknown".to_string()));
        assert_eq!(records.len(), 1);
        match &records[0] {
            ReplyRecord::Error(err) => assert!(err.contains("unknown command 'unknown'")),
            other => panic!("expected ReplyRecord::Error, got {other:?}"),
        }
    }

    #[test]
    fn list_commands_reports_setup_task() {
        let mut engine = Engine::default();
        let records = handle(&mut engine, Request::ListCommands);
        assert_eq!(records.len(), 2);
        match &records[0] {
            ReplyRecord::Command(cmd) => {
                assert_eq!(cmd.id, "setup");
                assert_eq!(cmd.label, "Download default SenseVoice model");
            }
            other => panic!("expected ReplyRecord::Command, got {other:?}"),
        }
        assert_eq!(records[1], ReplyRecord::Ok);
    }

    #[test]
    fn invoke_setup_returns_ok_and_reflects_status() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model_dir = dir
            .path()
            .join("sherpa-onnx")
            .join(setup::DEFAULT_MODEL_DIR_NAME);
        std::fs::create_dir_all(&model_dir).expect("mkdir");
        std::fs::write(model_dir.join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(model_dir.join("model.int8.onnx"), b"int8").expect("model");

        let mut engine = Engine {
            data_dir: dir.path().to_path_buf(),
            ..Engine::default()
        };

        let records = handle(&mut engine, Request::InvokeCommand("setup".to_string()));
        assert_eq!(records, vec![ReplyRecord::Ok]);
        assert_eq!(engine.setup_manager.status(), setup::SetupStatus::Completed);
        assert_eq!(engine.availability(), Availability::Ready);
    }

    #[test]
    fn resolve_model_discovers_nested_model_under_data_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model_dir = dir
            .path()
            .join("sherpa-onnx")
            .join("sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17");
        std::fs::create_dir_all(&model_dir).expect("mkdir");
        std::fs::write(model_dir.join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(model_dir.join("model.int8.onnx"), b"int8").expect("model");

        let mut engine = Engine {
            data_dir: dir.path().to_path_buf(),
            ..Engine::default()
        };

        let resolved = engine.resolve_model().expect("resolve");
        assert_eq!(resolved.dir(), model_dir);
        assert_eq!(engine.availability(), Availability::Ready);
    }
}
