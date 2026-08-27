//! Persistent user settings store (D-09 through D-14).
//!
//! `load()` is re-read on every capture hotkey press (D-11) so hand edits to
//! `config.json` apply immediately with no restart. Also the security
//! boundary (T-02-01/T-02-02) for the two config fields that flow into
//! filesystem paths: `base_filename` and `save_folder`.

use std::path::{Component, PathBuf};
use std::sync::OnceLock;

use crate::constants;
use crate::toast;

/// Characters rejected outright from a base filename (Windows-reserved plus
/// path separators). `.` is also stripped separately below so `..` can
/// never survive sanitization.
const INVALID_FILENAME_CHARS: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// Fallback base filename used whenever sanitization would otherwise yield
/// an empty string (D-09 default).
const DEFAULT_BASE_FILENAME: &str = "corvus";

/// Guards the once-per-process-run malformed-config toast (D-13).
static MALFORMED_TOASTED: OnceLock<()> = OnceLock::new();

/// Output image format, mirrored 1:1 from the `config.json` `format` string
/// so downstream code never string-matches (SAVE-06).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpg,
    Bmp,
    Webp,
}

impl Format {
    /// Parses a `config.json` format string case-insensitively. Any
    /// unrecognized value falls back to `Png` (D-10 default).
    pub fn from_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Format::Jpg,
            "bmp" => Format::Bmp,
            "webp" => Format::Webp,
            _ => Format::Png,
        }
    }

    /// File extension (without the leading dot) used for save filenames.
    pub fn ext(&self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpg => "jpg",
            Format::Bmp => "bmp",
            Format::Webp => "webp",
        }
    }
}

/// What happens when the user clicks a save-confirmation toast (D-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickAction {
    Dismiss,
    OpenFile,
    RevealExplorer,
}

impl ClickAction {
    /// Parses a `config.json` `toast_click_action` string. Any unrecognized
    /// value falls back to `Dismiss` (Phase 1 behavior, D-14 default).
    pub fn from_str(s: &str) -> Self {
        match s {
            "open_file" => ClickAction::OpenFile,
            "reveal_explorer" => ClickAction::RevealExplorer,
            _ => ClickAction::Dismiss,
        }
    }
}

/// The full persisted settings snapshot. `#[serde(default)]` means unknown
/// or missing keys never break parsing of an older/newer `config.json`
/// (forward compatibility -- config schema versioning guidance).
#[derive(serde::Serialize, serde::Deserialize, Clone)]
#[serde(default)]
pub struct Config {
    pub version: u32,
    pub base_filename: String,
    pub save_folder: String,
    pub format: String,
    pub jpg_quality: u8,
    pub toast_enabled: bool,
    pub clipboard_enabled: bool,
    pub toast_click_action: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            base_filename: DEFAULT_BASE_FILENAME.to_string(),
            save_folder: constants::default_capture_dir().to_string_lossy().into_owned(),
            format: "png".to_string(),
            jpg_quality: 85, // Claude's discretion within the SAVE-06 50-100 range
            toast_enabled: true,       // D-12
            clipboard_enabled: false,  // D-12
            toast_click_action: "dismiss".to_string(), // D-14
        }
    }
}

/// Resolves `%APPDATA%\CorvusCapture\config.json`, mirroring
/// `constants::default_capture_dir`'s env-var-based style.
pub fn config_path() -> PathBuf {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    PathBuf::from(appdata)
        .join(constants::APP_DATA_DIR_NAME)
        .join(constants::CONFIG_FILE_NAME)
}

/// Loads the config, implementing D-13 exactly:
/// - read succeeds + parse succeeds -> return the parsed config
/// - read succeeds + parse fails -> toast once per process run, return
///   defaults, never touch the user's malformed file
/// - read fails with `NotFound` -> build defaults, best-effort write them
///   out, return them
/// - read fails for any other reason (invalid UTF-8, permission/sharing
///   error) -> the file EXISTS but is unreadable; treat it like malformed
///   (toast once, use in-memory defaults) and never write over it
///
/// `jpg_quality` is clamped into 50..=100 on every path.
pub fn load() -> Config {
    let path = config_path();
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(_) => {
                if MALFORMED_TOASTED.set(()).is_ok() {
                    toast::show("config.json invalid — using defaults");
                }
                Config::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let cfg = Config::default();
            // Best-effort; capture still proceeds on defaults even if this
            // write fails (e.g. a not-yet-existing/unwritable %APPDATA%).
            let _ = save(&cfg);
            cfg
        }
        Err(_) => {
            // Unreadable-but-present file (invalid UTF-8, locked, EACCES):
            // D-13 forbids touching the user's file -- defaults only, no
            // write, same once-per-run toast as the malformed-parse path.
            if MALFORMED_TOASTED.set(()).is_ok() {
                toast::show("config.json invalid — using defaults");
            }
            Config::default()
        }
    };

    cfg.jpg_quality = cfg.jpg_quality.clamp(50, 100);
    cfg
}

/// Writes `cfg` as pretty-printed JSON, creating the parent directory if
/// needed.
pub fn save(cfg: &Config) -> std::io::Result<()> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(cfg)?;
    std::fs::write(&path, text)
}

/// Strips every character in `\ / : * ? " < > |` plus all `.` characters
/// (which also kills `..` traversal and stray extensions), trims ASCII
/// whitespace, and falls back to `"corvus"` if the result is empty
/// (T-02-01, ASVS V5).
pub fn sanitize_base_filename(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !INVALID_FILENAME_CHARS.contains(c) && *c != '.')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        DEFAULT_BASE_FILENAME.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolves `cfg.save_folder` into a safe destination directory. Falls back
/// to `constants::default_capture_dir()` if the configured value is empty,
/// relative, or contains a `..` component, rather than blindly joining an
/// attacker-shaped path (T-02-02, ASVS V12).
pub fn resolve_save_folder(cfg: &Config) -> PathBuf {
    if cfg.save_folder.trim().is_empty() {
        return constants::default_capture_dir();
    }

    let candidate = PathBuf::from(&cfg.save_folder);
    if !candidate.is_absolute() {
        return constants::default_capture_dir();
    }
    if candidate
        .components()
        .any(|c| matches!(c, Component::ParentDir))
    {
        return constants::default_capture_dir();
    }

    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_plain_name_unchanged() {
        assert_eq!(sanitize_base_filename("corvus"), "corvus");
    }

    #[test]
    fn sanitize_strips_traversal() {
        let result = sanitize_base_filename("../../evil");
        assert!(!result.contains('.'));
        assert!(!result.contains('/'));
        assert!(!result.contains('\\'));
        assert!(!result.contains(".."));
    }

    #[test]
    fn sanitize_strips_reserved_chars() {
        let result = sanitize_base_filename("bad:name*?");
        for c in INVALID_FILENAME_CHARS {
            assert!(!result.contains(*c));
        }
    }

    #[test]
    fn sanitize_blank_falls_back_to_default() {
        assert_eq!(sanitize_base_filename("   "), "corvus");
        assert_eq!(sanitize_base_filename(""), "corvus");
    }

    #[test]
    fn default_config_matches_locked_decisions() {
        let cfg = Config::default();
        assert_eq!(cfg.base_filename, "corvus");
        assert_eq!(Format::from_str(&cfg.format), Format::Png);
        assert_eq!(cfg.jpg_quality, 85);
        assert!(cfg.toast_enabled);
        assert!(!cfg.clipboard_enabled);
        assert_eq!(ClickAction::from_str(&cfg.toast_click_action), ClickAction::Dismiss);
        assert_eq!(cfg.version, 1);
    }

    #[test]
    fn format_from_str_case_insensitive() {
        assert_eq!(Format::from_str("png"), Format::Png);
        assert_eq!(Format::from_str("PNG"), Format::Png);
        assert_eq!(Format::from_str("jpg"), Format::Jpg);
        assert_eq!(Format::from_str("JPG"), Format::Jpg);
        assert_eq!(Format::from_str("bmp"), Format::Bmp);
        assert_eq!(Format::from_str("BMP"), Format::Bmp);
        assert_eq!(Format::from_str("webp"), Format::Webp);
        assert_eq!(Format::from_str("WEBP"), Format::Webp);
        assert_eq!(Format::from_str("unknown"), Format::Png);
    }

    #[test]
    fn format_ext_matches_variant() {
        assert_eq!(Format::Png.ext(), "png");
        assert_eq!(Format::Jpg.ext(), "jpg");
        assert_eq!(Format::Bmp.ext(), "bmp");
        assert_eq!(Format::Webp.ext(), "webp");
    }

    #[test]
    fn jpg_quality_clamped_on_load_shape() {
        // load() itself touches the filesystem; exercise the clamp logic
        // directly against the same range it applies.
        let mut cfg = Config::default();
        cfg.jpg_quality = 10;
        cfg.jpg_quality = cfg.jpg_quality.clamp(50, 100);
        assert_eq!(cfg.jpg_quality, 50);

        cfg.jpg_quality = 250;
        cfg.jpg_quality = cfg.jpg_quality.clamp(50, 100);
        assert_eq!(cfg.jpg_quality, 100);
    }
}
