//! User configuration loaded from `~/.config/fermi-term/config.toml`.
//!
//! All fields have sane defaults so the file is entirely optional.

use serde::Deserialize;
use std::path::PathBuf;

/// Top-level configuration structure.
///
/// Deserialised from TOML. Missing fields fall back to [`Default`].
#[derive(Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Font size in points.
    pub font_size: f32,
    /// Shell to spawn (defaults to `$SHELL` env var, then `/bin/sh`).
    pub shell: String,
    /// Default foreground colour as `[R, G, B]`.
    pub fg: [u8; 3],
    /// Default background colour as `[R, G, B]`.
    pub bg: [u8; 3],
    /// Cursor colour as `[R, G, B]`.
    pub cursor_color: [u8; 3],
    /// Maximum number of scrollback lines retained in memory.
    pub scrollback_lines: usize,
    /// Initial window width in pixels.
    pub window_width: u32,
    /// Initial window height in pixels.
    pub window_height: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
            fg: [200, 200, 200],
            bg: [14, 14, 26],
            cursor_color: [220, 220, 100],
            scrollback_lines: 10_000,
            window_width: 1200,
            window_height: 800,
        }
    }
}

impl Config {
    /// Load config from `~/.config/fermi-term/config.toml`.
    ///
    /// Returns [`Config::default`] if the file does not exist or fails to parse.
    pub fn load() -> Self {
        let path = Self::config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        toml::from_str(&text).unwrap_or_else(|err| {
            eprintln!(
                "[fermi-term] Warning: failed to parse config at {}: {err}",
                path.display()
            );
            Self::default()
        })
    }

    fn config_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(".config/fermi-term/config.toml")
    }
}
