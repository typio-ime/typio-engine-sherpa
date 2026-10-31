//! Background model acquisition and setup worker for typio-engine-sherpa.
//!
//! Autonomous, domain-cohesive model provisioning owned entirely by the
//! engine. Downloads, verifies, and unpacks the default SenseVoice Small int8
//! model on an isolated detached thread without blocking the Engine Protocol
//! event loop.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};

use crate::model::ModelFiles;

/// Canonical directory name under `<data_dir>/sherpa-onnx/`.
pub const DEFAULT_MODEL_DIR_NAME: &str = "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17";
/// Source download URL.
pub const DEFAULT_MODEL_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-int8-2024-07-17.tar.bz2";
/// Expected SHA-256 checksum of the upstream archive.
pub const DEFAULT_MODEL_SHA256: &str =
    "7d1efa2138a65b0b488df37f8b89e3d91a60676e416f515b952358d83dfd347e";

/// Lifecycle state of the background setup task.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SetupStatus {
    /// No setup has been requested.
    #[default]
    Idle,
    /// Model archive is downloading.
    Downloading,
    /// Archive is being verified and extracted.
    Extracting,
    /// Model is fully verified on disk and ready for inference.
    Completed,
    /// An error occurred during acquisition.
    Failed(String),
}

/// Thread-safe manager for background model provisioning.
#[derive(Default)]
pub struct SetupManager {
    status: Arc<RwLock<SetupStatus>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl SetupManager {
    /// Current setup status.
    pub fn status(&self) -> SetupStatus {
        self.status
            .read()
            .map(|s| s.clone())
            .unwrap_or(SetupStatus::Idle)
    }

    /// Whether a background setup worker is currently in flight.
    pub fn is_running(&self) -> bool {
        matches!(
            self.status(),
            SetupStatus::Downloading | SetupStatus::Extracting
        )
    }

    /// Spawn a detached background worker thread to acquire the default model.
    ///
    /// Returns `Ok(true)` if a new worker was spawned, or `Ok(false)` if setup
    /// is already running or previously completed.
    pub fn start_setup(&self, data_dir: PathBuf) -> Result<bool, String> {
        let mut status_lock = self
            .status
            .write()
            .map_err(|e| format!("setup status lock poisoned: {e}"))?;

        if matches!(
            *status_lock,
            SetupStatus::Downloading | SetupStatus::Extracting
        ) {
            return Ok(false);
        }

        let target_dir = data_dir.join("sherpa-onnx").join(DEFAULT_MODEL_DIR_NAME);
        if ModelFiles::discover(&target_dir).is_ok() {
            *status_lock = SetupStatus::Completed;
            return Ok(false);
        }

        *status_lock = SetupStatus::Downloading;
        let status_arc = Arc::clone(&self.status);

        let handle = std::thread::Builder::new()
            .name("sherpa-setup".to_string())
            .spawn(move || {
                if let Err(error) = run_setup_pipeline(&data_dir, &status_arc) {
                    eprintln!("[sherpa-onnx] setup failed: {error}");
                    if let Ok(mut status) = status_arc.write() {
                        *status = SetupStatus::Failed(error);
                    }
                } else {
                    eprintln!("[sherpa-onnx] setup completed successfully");
                    if let Ok(mut status) = status_arc.write() {
                        *status = SetupStatus::Completed;
                    }
                }
            })
            .map_err(|e| format!("failed to spawn background setup thread: {e}"))?;

        if let Ok(mut worker_lock) = self.worker.lock() {
            *worker_lock = Some(handle);
        }

        Ok(true)
    }
}

/// Execute the download, verification, extraction, and installation pipeline.
fn run_setup_pipeline(data_dir: &Path, status: &Arc<RwLock<SetupStatus>>) -> Result<(), String> {
    let base_dir = data_dir.join("sherpa-onnx");
    let target_dir = base_dir.join(DEFAULT_MODEL_DIR_NAME);

    // If target already has valid model files, nothing to do.
    if ModelFiles::discover(&target_dir).is_ok() {
        return Ok(());
    }

    let temp_dir = tempfile::Builder::new()
        .prefix("typio-sherpa-setup-")
        .tempdir()
        .map_err(|e| format!("failed to create temp directory: {e}"))?;

    let archive_path = temp_dir
        .path()
        .join(format!("{DEFAULT_MODEL_DIR_NAME}.tar.bz2"));

    eprintln!("[sherpa-onnx] downloading default model from {DEFAULT_MODEL_URL}...");
    let curl_status = Command::new("curl")
        .args([
            "-fSL",
            "--retry",
            "3",
            "-o",
            archive_path.to_str().unwrap(),
            DEFAULT_MODEL_URL,
        ])
        .status()
        .map_err(|e| format!("curl invocation failed: {e}. Is curl installed?"))?;

    if !curl_status.success() {
        return Err(format!("curl failed with status {curl_status}"));
    }

    if let Ok(mut lock) = status.write() {
        *lock = SetupStatus::Extracting;
    }

    // SHA-256 integrity check.
    if let Ok(output) = Command::new("sha256sum").arg(&archive_path).output()
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let calculated_hash = stdout.split_whitespace().next().unwrap_or("");
        if calculated_hash != DEFAULT_MODEL_SHA256 {
            return Err(format!(
                "SHA-256 mismatch! Expected {DEFAULT_MODEL_SHA256}, got {calculated_hash}"
            ));
        }
        eprintln!("[sherpa-onnx] SHA-256 integrity verified successfully.");
    }

    eprintln!("[sherpa-onnx] extracting model archive...");
    let tar_status = Command::new("tar")
        .args([
            "xf",
            archive_path.to_str().unwrap(),
            "-C",
            temp_dir.path().to_str().unwrap(),
        ])
        .status()
        .map_err(|e| format!("tar invocation failed: {e}. Is tar installed?"))?;

    if !tar_status.success() {
        return Err(format!("tar extraction failed with status {tar_status}"));
    }

    let extracted_model_dir = temp_dir.path().join(DEFAULT_MODEL_DIR_NAME);
    let extracted_model_file = extracted_model_dir.join("model.int8.onnx");
    let extracted_tokens_file = extracted_model_dir.join("tokens.txt");

    if !extracted_model_file.is_file() || !extracted_tokens_file.is_file() {
        return Err(format!(
            "extracted archive did not contain expected model.int8.onnx and tokens.txt under {}",
            extracted_model_dir.display()
        ));
    }

    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("failed to create {}: {e}", target_dir.display()))?;

    std::fs::copy(&extracted_model_file, target_dir.join("model.int8.onnx"))
        .map_err(|e| format!("failed to copy model.int8.onnx: {e}"))?;
    std::fs::copy(&extracted_tokens_file, target_dir.join("tokens.txt"))
        .map_err(|e| format!("failed to copy tokens.txt: {e}"))?;

    // Substantive verification.
    ModelFiles::discover(&target_dir)
        .map_err(|e| format!("installed model verification failed: {e}"))?;

    eprintln!(
        "[sherpa-onnx] model installed and verified at {}",
        target_dir.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_starts_idle() {
        let manager = SetupManager::default();
        assert_eq!(manager.status(), SetupStatus::Idle);
        assert!(!manager.is_running());
    }

    #[test]
    fn existing_model_completes_immediately() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model_dir = dir.path().join("sherpa-onnx").join(DEFAULT_MODEL_DIR_NAME);
        std::fs::create_dir_all(&model_dir).expect("mkdir");
        std::fs::write(model_dir.join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(model_dir.join("model.int8.onnx"), b"int8").expect("model");

        let manager = SetupManager::default();
        let spawned = manager
            .start_setup(dir.path().to_path_buf())
            .expect("setup");
        assert!(!spawned);
        assert_eq!(manager.status(), SetupStatus::Completed);
    }
}
