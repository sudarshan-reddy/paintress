use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::discovery::DisplayInfo;
use crate::error::{PaintressError, Result};
use crate::image::{Rotation, DISPLAY_HEIGHT, DISPLAY_WIDTH};

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

    /// Native panel width in pixels, as reported over mDNS.
    /// Omitted in configs written before mixed-size fleets were supported —
    /// falls back to the compiled-in default. Refreshed on every discover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,

    /// Native panel height in pixels, as reported over mDNS.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl DisplayConfig {
    /// Native panel dimensions, falling back to the compiled-in default for
    /// configs written before panel size was recorded.
    pub fn dims(&self) -> (u32, u32) {
        (
            self.width.unwrap_or(DISPLAY_WIDTH),
            self.height.unwrap_or(DISPLAY_HEIGHT),
        )
    }
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
                width: Some(d.width),
                height: Some(d.height),
            })
            .collect();

        eprintln!(
            "Auto-generated {CONFIG_FILE} with {cols} display(s) in a {cols}x1 grid.\n\
             Edit the file to change names, positions, or orientations."
        );

        Config { display }
    }

    /// Merge newly discovered displays into an existing config.
    /// - Existing entries keep their name, position and mounting.
    /// - Panel size is refreshed from discovery (it's hardware, not preference).
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
                    width: Some(d.width),
                    height: Some(d.height),
                });
                next_col += 1;
            }
        }

        // Backfill/refresh panel size from the network. Configs written before
        // this field existed have None here and pick up their real size now.
        let sizes: HashMap<&str, (u32, u32)> = displays
            .iter()
            .map(|d| (d.id.as_str(), (d.width, d.height)))
            .collect();

        for dc in &mut self.display {
            let Some(&(w, h)) = sizes.get(dc.serial.as_str()) else {
                continue; // offline — leave whatever the config already says
            };
            if dc.width == Some(w) && dc.height == Some(h) {
                continue;
            }
            if dc.width.is_some() || dc.height.is_some() {
                let (ow, oh) = dc.dims();
                eprintln!("  {}: panel size {ow}x{oh} -> {w}x{h}", dc.serial);
            }
            dc.width = Some(w);
            dc.height = Some(h);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A config as written by versions before panel size was recorded.
    const LEGACY_TOML: &str = r#"
[[display]]
serial = "f8c0a8"
name = "kitchen"
col = 1
row = 0
mounted = "portrait-left"

[[display]]
serial = "f8fab4"
name = "hallway"
col = 0
row = 0
mounted = "portrait-right"
"#;

    fn info(id: &str, width: u32, height: u32) -> DisplayInfo {
        DisplayInfo {
            id: id.to_owned(),
            ip: "192.0.2.1".to_owned(),
            port: 80,
            hostname: format!("eink-{id}.local"),
            width,
            height,
        }
    }

    #[test]
    fn legacy_config_loads_and_falls_back_to_default_size() {
        let cfg: Config = toml::from_str(LEGACY_TOML).unwrap();
        assert_eq!(cfg.display.len(), 2);
        for dc in &cfg.display {
            assert_eq!(dc.width, None);
            assert_eq!(dc.height, None);
            assert_eq!(dc.dims(), (DISPLAY_WIDTH, DISPLAY_HEIGHT));
        }
    }

    #[test]
    fn legacy_config_round_trips_without_gaining_size_fields() {
        let cfg: Config = toml::from_str(LEGACY_TOML).unwrap();
        let out = toml::to_string_pretty(&cfg).unwrap();
        assert!(!out.contains("width"), "unexpected width in:\n{out}");
        assert!(!out.contains("height"), "unexpected height in:\n{out}");
    }

    #[test]
    fn merge_backfills_size_but_preserves_user_fields() {
        let mut cfg: Config = toml::from_str(LEGACY_TOML).unwrap();
        cfg.merge_discovered(&[info("f8c0a8", 1200, 1600), info("f8fab4", 800, 480)]);

        let kitchen = cfg.display.iter().find(|d| d.serial == "f8c0a8").unwrap();
        assert_eq!(kitchen.dims(), (1200, 1600));
        // Name, position and mounting are the user's — merge must not touch them.
        assert_eq!(kitchen.name, "kitchen");
        assert_eq!((kitchen.col, kitchen.row), (1, 0));
        assert_eq!(kitchen.mounted, Mounting::PortraitLeft);

        let hallway = cfg.display.iter().find(|d| d.serial == "f8fab4").unwrap();
        assert_eq!(hallway.dims(), (800, 480));
    }

    #[test]
    fn merge_leaves_offline_displays_alone() {
        let mut cfg: Config = toml::from_str(LEGACY_TOML).unwrap();
        cfg.display[0].width = Some(1200);
        cfg.display[0].height = Some(1600);

        // Only the second display answers this round.
        cfg.merge_discovered(&[info("f8fab4", 800, 480)]);

        assert_eq!(cfg.display[0].dims(), (1200, 1600));
        assert_eq!(cfg.display.len(), 2, "offline display must not be dropped");
    }

    #[test]
    fn merge_appends_new_display_with_its_size() {
        let mut cfg: Config = toml::from_str(LEGACY_TOML).unwrap();
        cfg.merge_discovered(&[info("aabbcc", 1200, 1600)]);

        let added = cfg.display.iter().find(|d| d.serial == "aabbcc").unwrap();
        assert_eq!(added.dims(), (1200, 1600));
        assert_eq!(added.col, 2, "should land right of the existing max col");
    }

    #[test]
    fn from_discovered_records_size() {
        let cfg = Config::from_discovered(&[info("aabbcc", 1200, 1600)]);
        assert_eq!(cfg.display[0].width, Some(1200));
        assert_eq!(cfg.display[0].height, Some(1600));
    }
}
