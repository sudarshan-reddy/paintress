use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::discovery::DisplayInfo;
use crate::error::{PaintressError, Result};
use crate::image::Rotation;

const CONFIG_FILE: &str = "paintress.toml";

/// Physical mounting orientation of a display.
/// Describes which edge of the display is at the top.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Mounting {
    #[default]
    Landscape,
    /// Portrait with the left edge at the top
    PortraitLeft,
    /// Portrait with the right edge at the top
    PortraitRight,
    /// Upside down landscape
    UpsideDown,
}

impl Mounting {
    /// The rotation needed to compensate for this physical mounting.
    pub fn rotation(self) -> Rotation {
        match self {
            Mounting::Landscape => Rotation::None,
            Mounting::PortraitLeft => Rotation::Cw90,
            Mounting::PortraitRight => Rotation::Ccw90,
            Mounting::UpsideDown => Rotation::Flip180,
        }
    }

    /// Canvas dimensions for this mounting, given native display (w, h).
    pub fn canvas_dims(self, native_w: u32, native_h: u32) -> (u32, u32) {
        self.rotation().canvas_dims(native_w, native_h)
    }
}

/// A display entry in the config file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// mDNS serial / ID (e.g. "eink-abc123")
    pub serial: String,

    /// Human-friendly name (editable by user)
    #[serde(default)]
    pub name: String,

    /// Grid column (0-indexed, left to right)
    #[serde(default)]
    pub col: u32,

    /// Grid row (0-indexed, top to bottom)
    #[serde(default)]
    pub row: u32,

    /// How the display is physically mounted
    #[serde(default)]
    pub mounted: Mounting,
}

/// Top-level config.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub display: Vec<DisplayConfig>,
}

impl Config {
    /// Load config from `paintress.toml` in the current directory.
    pub fn load() -> Result<Option<Config>> {
        let path = PathBuf::from(CONFIG_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)?;
        let config: Config =
            toml::from_str(&text).map_err(|e| PaintressError::Generic(format!("bad config: {e}")))?;
        Ok(Some(config))
    }

    /// Save config to `paintress.toml`.
    pub fn save(&self) -> Result<()> {
        let text =
            toml::to_string_pretty(self).map_err(|e| PaintressError::Generic(e.to_string()))?;
        std::fs::write(CONFIG_FILE, text)?;
        Ok(())
    }

    /// Generate a default config from discovered displays.
    /// Arranges them in a single-row grid, all landscape, sorted by ID.
    pub fn from_discovered(displays: &[DisplayInfo]) -> Config {
        let cols = displays.len() as u32;
        let display = displays
            .iter()
            .enumerate()
            .map(|(i, d)| DisplayConfig {
                serial: d.id.clone(),
                name: d.id.clone(),
                col: i as u32,
                row: 0,
                mounted: Mounting::default(),
            })
            .collect();

        eprintln!(
            "Auto-generated {CONFIG_FILE} with {cols} display(s) in a {cols}x1 grid.\n\
             Edit the file to change names, positions, or orientations."
        );

        Config { display }
    }

    /// Merge newly discovered displays into an existing config.
    /// - Existing entries are kept as-is.
    /// - New displays get appended with auto-assigned positions.
    pub fn merge_discovered(&mut self, displays: &[DisplayInfo]) {
        let known: HashSet<String> = self.display.iter().map(|d| d.serial.clone()).collect();

        let max_col = self.display.iter().map(|d| d.col).max().unwrap_or(0);
        let mut next_col = max_col + 1;

        for d in displays {
            if !known.contains(&d.id) {
                eprintln!("  New display found: {} — adding to config", d.id);
                self.display.push(DisplayConfig {
                    serial: d.id.clone(),
                    name: d.id.clone(),
                    col: next_col,
                    row: 0,
                    mounted: Mounting::default(),
                });
                next_col += 1;
            }
        }
    }

    /// Resolve config entries against discovered displays on the network.
    /// Returns matched pairs. Errors if a configured display is missing from network.
    pub fn resolve<'a>(
        &'a self,
        discovered: &'a [DisplayInfo],
    ) -> Result<Vec<(&'a DisplayConfig, &'a DisplayInfo)>> {
        let by_id: HashMap<&str, &DisplayInfo> =
            discovered.iter().map(|d| (d.id.as_str(), d)).collect();

        let mut resolved = Vec::new();
        let mut missing = Vec::new();

        for dc in &self.display {
            if let Some(di) = by_id.get(dc.serial.as_str()) {
                resolved.push((dc, *di));
            } else {
                missing.push(&dc.serial);
            }
        }

        if !missing.is_empty() {
            eprintln!(
                "warning: {} configured display(s) not found on network: {}",
                missing.len(),
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        if resolved.is_empty() {
            return Err(PaintressError::NoDisplaysFound);
        }

        Ok(resolved)
    }
}
