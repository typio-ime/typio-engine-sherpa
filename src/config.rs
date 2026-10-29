//! Reading the engine's own configuration out of the host's `core.toml`.
//!
//! The host hands each engine a `config_dir` in `HOST_HELLO` and owns the file
//! itself; an engine reads `engines.<name>.<key>` from it on load and again after
//! `ReloadConfig`. That is the whole contract ([`Config::engine_value`] on the
//! host side).
//!
//! # Why document access instead of deserialisation
//!
//! `core.toml` is the *host's* configuration tree. It contains keys for every
//! engine, every keyboard shortcut, and every host feature — a shape this engine
//! must not model. Deserialising it into a struct would fail on (or silently drop)
//! every field that is not ours, so the file is walked as a TOML document and only
//! `engines.<name>.<key>` is read.
//!
//! # Failure policy
//!
//! A missing file, an unreadable file, or a missing key is not an error here: the
//! engine has documented defaults for both keys, and a stock install ships no
//! configuration at all. What *is* an error is a key whose value has the wrong
//! type — `model = 3` must be reported, not ignored, because silently falling back
//! to auto-detection would transcribe with a model the user did not choose.

use std::path::Path;

/// Configuration keys this engine owns.
pub const MODEL_KEY: &str = "model";
/// Recognition language hint.
pub const LANGUAGE_KEY: &str = "language";

/// Values read from `engines.<name>.*`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EngineConfig {
    /// Explicit model directory, or `None` to auto-detect.
    pub model: Option<String>,
    /// Language hint, or `None` to let the model decide.
    pub language: Option<String>,
}

impl EngineConfig {
    /// Read this engine's section out of `<config_dir>/core.toml`.
    ///
    /// Absence of the file, the section, or an individual key yields the default
    /// for that key. A wrong-typed value is reported so a mistyped configuration
    /// cannot silently change behaviour.
    ///
    /// # Errors
    /// Returns a message when the file exists but cannot be read or parsed, or
    /// when one of this engine's keys holds a non-string value.
    pub fn read(config_dir: &Path, engine_name: &str) -> Result<Self, String> {
        let path = config_dir.join("core.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            // No configuration file is the normal first-run state.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(format!("read {}: {error}", path.display())),
        };
        Self::parse(&text, engine_name).map_err(|error| format!("{}: {error}", path.display()))
    }

    /// Parse the engine section out of TOML text.
    ///
    /// # Errors
    /// Returns a message for malformed TOML or a wrong-typed key.
    pub fn parse(text: &str, engine_name: &str) -> Result<Self, String> {
        let document = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|error| format!("invalid TOML: {error}"))?;
        let Some(section) = document
            .get("engines")
            .and_then(|engines| engines.get(engine_name))
        else {
            return Ok(Self::default());
        };

        Ok(Self {
            model: string_value(section, MODEL_KEY)?,
            language: string_value(section, LANGUAGE_KEY)?,
        })
    }
}

/// Read one string field, tolerating absence and rejecting other types.
fn string_value(section: &toml_edit::Item, key: &str) -> Result<Option<String>, String> {
    let Some(item) = section.get(key) else {
        return Ok(None);
    };
    let value = item.as_str().ok_or_else(|| {
        format!(
            "`engines.*.{key}` must be a string, found {}",
            item.type_name()
        )
    })?;
    // An explicitly empty string means "unset", which is how the host writes a
    // cleared key.
    Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<EngineConfig, String> {
        EngineConfig::parse(text, "sherpa-onnx")
    }

    #[test]
    fn absent_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = EngineConfig::read(dir.path(), "sherpa-onnx").expect("read");
        assert_eq!(config, EngineConfig::default());
    }

    #[test]
    fn absent_section_yields_defaults() {
        let config = parse("[keyboard]\nengine = \"rime\"\n").expect("parse");
        assert_eq!(config, EngineConfig::default());
    }

    #[test]
    fn reads_both_keys() {
        let config = parse("[engines.sherpa-onnx]\nmodel = \"sense-voice\"\nlanguage = \"zh\"\n")
            .expect("parse");
        assert_eq!(config.model.as_deref(), Some("sense-voice"));
        assert_eq!(config.language.as_deref(), Some("zh"));
    }

    #[test]
    fn ignores_other_engines_and_host_keys() {
        let config = parse(
            "[engines.rime]\nmodel = \"nope\"\n\
             [engines.sherpa-onnx]\nmodel = \"mine\"\n\
             [shortcuts]\nvoice_ptt = \"Super+v\"\n",
        )
        .expect("parse");
        assert_eq!(config.model.as_deref(), Some("mine"));
        assert_eq!(config.language, None);
    }

    #[test]
    fn an_empty_value_means_unset() {
        let config = parse("[engines.sherpa-onnx]\nmodel = \"  \"\n").expect("parse");
        assert_eq!(config.model, None);
    }

    #[test]
    fn rejects_a_non_string_model() {
        let error = parse("[engines.sherpa-onnx]\nmodel = 3\n").expect_err("must reject");
        assert!(error.contains("must be a string"), "{error}");
        assert!(error.contains("integer"), "{error}");
    }

    #[test]
    fn rejects_malformed_toml() {
        let error = parse("this is not toml").expect_err("must reject");
        assert!(error.contains("invalid TOML"), "{error}");
    }

    #[test]
    fn reads_from_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("core.toml"),
            "[engines.sherpa-onnx]\nlanguage = \"ja\"\n",
        )
        .expect("write");
        let config = EngineConfig::read(dir.path(), "sherpa-onnx").expect("read");
        assert_eq!(config.language.as_deref(), Some("ja"));
    }
}
