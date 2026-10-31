//! On-disk model layout discovery.
//!
//! This module contains **no FFI**. Model-layout rules are pure filesystem
//! logic, so keeping them out of the recognizer module means they can be
//! exercised on any machine — including one with no sherpa-onnx installed, where
//! the recognizer itself cannot even be linked. That separation is deliberate:
//! the most common real-world failure is "the model directory is not where or
//! what the engine expects", and that failure deserves a test that always runs.

use std::path::{Path, PathBuf};

/// Which model family a directory holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// SenseVoice: `model.int8.onnx` (or `model.onnx`) plus `tokens.txt`.
    SenseVoice,
}

impl ModelKind {
    /// Stable label used in logs and error text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SenseVoice => "sense_voice",
        }
    }
}

/// Why a model directory could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelError {
    /// No recognisable layout under the given path.
    NotFound {
        /// Directory that was inspected.
        dir: PathBuf,
    },
    /// More than one candidate subdirectory, so the choice would be arbitrary.
    Ambiguous {
        /// Directory that was inspected.
        dir: PathBuf,
        /// How many candidates were found.
        candidates: usize,
    },
}

impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { dir } => write!(
                f,
                "no recognisable model in {}: expected model.int8.onnx or model.onnx \
                 together with tokens.txt",
                dir.display()
            ),
            Self::Ambiguous { dir, candidates } => write!(
                f,
                "{} candidate model subdirectories under {}; point at one explicitly",
                candidates,
                dir.display()
            ),
        }
    }
}

impl std::error::Error for ModelError {}

/// The files that make up an on-disk model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFiles {
    dir: PathBuf,
    kind: ModelKind,
    model: PathBuf,
    tokens: PathBuf,
}

impl ModelFiles {
    /// Resolve a model directory into its files.
    ///
    /// Two shapes are accepted:
    ///
    /// 1. **A model directory** holding `model.int8.onnx` (or `model.onnx`) and
    ///    `tokens.txt`.
    /// 2. **A base directory** containing exactly one recognisable model
    ///    subdirectory, which is how the upstream release archive and the
    ///    installer lay a model out.
    ///
    /// # Errors
    /// [`ModelError::NotFound`] when neither shape matches, or
    /// [`ModelError::Ambiguous`] when a base directory offers more than one
    /// candidate — guessing there would silently transcribe with the wrong model.
    pub fn discover(input: &Path) -> Result<Self, ModelError> {
        if let Some(files) = Self::probe(input) {
            return Ok(files);
        }

        let mut candidates: Vec<ModelFiles> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(input) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                if let Some(files) = Self::probe(&path) {
                    candidates.push(files);
                }
            }
        }
        match candidates.len() {
            1 => Ok(candidates.remove(0)),
            0 => Err(ModelError::NotFound {
                dir: input.to_path_buf(),
            }),
            n => Err(ModelError::Ambiguous {
                dir: input.to_path_buf(),
                candidates: n,
            }),
        }
    }

    /// Model directory that was resolved.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Model family.
    pub fn kind(&self) -> ModelKind {
        self.kind
    }

    /// Path to the ONNX model artifact.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub fn model_path(&self) -> &Path {
        &self.model
    }

    /// Path to the token table.
    #[cfg_attr(not(feature = "native"), allow(dead_code))]
    pub fn tokens_path(&self) -> &Path {
        &self.tokens
    }

    fn probe(dir: &Path) -> Option<Self> {
        let tokens = dir.join("tokens.txt");
        if !tokens.is_file() {
            return None;
        }
        // Prefer the int8 artifact: it is what the installer ships and is ~4x
        // smaller for the same accuracy on this family.
        let model = ["model.int8.onnx", "model.onnx"]
            .iter()
            .map(|name| dir.join(name))
            .find(|candidate| candidate.is_file())?;
        Some(Self {
            dir: dir.to_path_buf(),
            kind: ModelKind::SenseVoice,
            model,
            tokens,
        })
    }

    /// List all model subdirectory names under `base_dir`.
    pub fn list_models(base_dir: &Path) -> Vec<String> {
        let mut models = Vec::new();
        if let Ok(entries) = std::fs::read_dir(base_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && Self::probe(&path).is_some()
                    && let Some(name) = path.file_name().and_then(|n| n.to_str())
                {
                    models.push(name.to_string());
                }
            }
        }
        models.sort();
        models
    }

    /// Discover models across multiple XDG-compliant search paths.
    pub fn discover_in_search_dirs(dirs: &[PathBuf]) -> Result<Self, ModelError> {
        for dir in dirs {
            if let Ok(files) = Self::discover(dir) {
                return Ok(files);
            }
        }
        Err(ModelError::NotFound {
            dir: dirs.first().cloned().unwrap_or_default(),
        })
    }

    /// Enumerate all recognizable models across multiple search paths.
    pub fn list_models_in_search_dirs(dirs: &[PathBuf]) -> Vec<String> {
        let mut models = Vec::new();
        for dir in dirs {
            for m in Self::list_models(dir) {
                if !models.contains(&m) {
                    models.push(m);
                }
            }
        }
        models.sort();
        models
    }
}

/// Canonical XDG-compliant search directories for models.
pub fn model_search_dirs(custom_data_dir: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(custom) = custom_data_dir {
        dirs.push(custom.join("sherpa-onnx"));
    }
    if let Some(xdg_data) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        let p = xdg_data.join("typio/sherpa-onnx");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    } else if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let p = home.join(".local/share/typio/sherpa-onnx");
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }

    let system_dirs = [
        PathBuf::from("/usr/local/share/typio/sherpa-onnx"),
        PathBuf::from("/usr/share/typio/sherpa-onnx"),
    ];
    for sys in system_dirs {
        if !dirs.contains(&sys) {
            dirs.push(sys);
        }
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_model(dir: &Path) {
        std::fs::create_dir_all(dir).expect("mkdir");
        std::fs::write(dir.join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(dir.join("model.int8.onnx"), b"stub").expect("model");
    }

    #[test]
    fn rejects_a_directory_without_tokens() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = ModelFiles::discover(dir.path()).expect_err("must reject");
        assert!(matches!(error, ModelError::NotFound { .. }));
        assert!(error.to_string().contains("tokens.txt"));
    }

    #[test]
    fn rejects_a_directory_with_tokens_but_no_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tokens.txt"), "<unk> 0\n").expect("tokens");
        assert!(matches!(
            ModelFiles::discover(dir.path()),
            Err(ModelError::NotFound { .. })
        ));
    }

    #[test]
    fn resolves_a_direct_model_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_model(dir.path());
        let files = ModelFiles::discover(dir.path()).expect("discover");
        assert_eq!(files.dir(), dir.path());
        assert_eq!(files.kind(), ModelKind::SenseVoice);
        assert!(files.model_path().ends_with("model.int8.onnx"));
        assert!(files.tokens_path().ends_with("tokens.txt"));
    }

    #[test]
    fn resolves_a_nested_model_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        let model_dir = root.path().join("sense-voice-int8");
        write_model(&model_dir);
        let files = ModelFiles::discover(root.path()).expect("discover");
        assert_eq!(files.dir(), model_dir);
    }

    #[test]
    fn prefers_the_int8_artifact_over_the_float_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(dir.path().join("model.onnx"), b"float").expect("float");
        std::fs::write(dir.path().join("model.int8.onnx"), b"int8").expect("int8");
        let files = ModelFiles::discover(dir.path()).expect("discover");
        assert!(files.model_path().ends_with("model.int8.onnx"));
    }

    #[test]
    fn falls_back_to_the_float_model_when_int8_is_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tokens.txt"), "<unk> 0\n").expect("tokens");
        std::fs::write(dir.path().join("model.onnx"), b"float").expect("float");
        let files = ModelFiles::discover(dir.path()).expect("discover");
        assert!(files.model_path().ends_with("model.onnx"));
    }

    #[test]
    fn refuses_to_guess_between_two_candidate_models() {
        let root = tempfile::tempdir().expect("tempdir");
        write_model(&root.path().join("a"));
        write_model(&root.path().join("b"));
        let error = ModelFiles::discover(root.path()).expect_err("must reject");
        assert!(matches!(error, ModelError::Ambiguous { candidates: 2, .. }));
        assert!(error.to_string().contains("2 candidate"));
    }

    #[test]
    fn ignores_unrelated_subdirectories() {
        let root = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(root.path().join("readme")).expect("mkdir");
        write_model(&root.path().join("model"));
        let files = ModelFiles::discover(root.path()).expect("discover");
        assert!(files.dir().ends_with("model"));
    }

    #[test]
    fn list_models_enumerates_valid_directories() {
        let root = tempfile::tempdir().expect("tempdir");
        write_model(&root.path().join("model-a"));
        write_model(&root.path().join("model-b"));
        std::fs::create_dir_all(root.path().join("not-a-model")).expect("mkdir");
        let list = ModelFiles::list_models(root.path());
        assert_eq!(list, vec!["model-a", "model-b"]);
    }

    #[test]
    fn discover_in_search_dirs_finds_first_valid_location() {
        let empty_root = tempfile::tempdir().expect("tempdir");
        let populated_root = tempfile::tempdir().expect("tempdir");
        write_model(&populated_root.path().join("model-c"));

        let dirs = vec![
            empty_root.path().to_path_buf(),
            populated_root.path().to_path_buf(),
        ];
        let found = ModelFiles::discover_in_search_dirs(&dirs).expect("discover");
        assert_eq!(found.dir(), populated_root.path().join("model-c"));
    }
}
