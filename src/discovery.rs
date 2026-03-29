use serde::{Deserialize, Serialize};

/// Information about a discovered e-ink display.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: String,
    pub ip: String,
    pub port: u16,
    pub hostname: String,
    pub width: u32,
    pub height: u32,
}
