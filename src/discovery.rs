use std::collections::HashMap;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::{Deserialize, Serialize};

use crate::error::{PaintressError, Result};
use crate::image::{DISPLAY_HEIGHT, DISPLAY_WIDTH};

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

/// Discover e-ink displays via mDNS (`_eink._tcp.local.`).
pub fn discover(timeout: Duration) -> Result<Vec<DisplayInfo>> {
    let mdns = ServiceDaemon::new().map_err(|e| PaintressError::Generic(e.to_string()))?;
    let receiver = mdns
        .browse("_eink._tcp.local.")
        .map_err(|e| PaintressError::Generic(e.to_string()))?;

    let mut displays: HashMap<String, DisplayInfo> = HashMap::new();
    let deadline = std::time::Instant::now() + timeout;

    eprintln!("Scanning for displays ({:.0}s)...", timeout.as_secs_f64());

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match receiver.recv_timeout(remaining) {
            Ok(ServiceEvent::ServiceResolved(info)) => {
                let ip = info
                    .get_addresses()
                    .iter()
                    .find(|a| a.is_ipv4())
                    .map(|a| a.to_string());
                let Some(ip) = ip else { continue };

                let props = info.get_properties();
                let id = props
                    .get_property_val_str("id")
                    .unwrap_or_else(|| info.get_fullname().split('.').next().unwrap_or("unknown"))
                    .to_owned();

                let width = props
                    .get_property_val_str("width")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DISPLAY_WIDTH);
                let height = props
                    .get_property_val_str("height")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DISPLAY_HEIGHT);

                displays.insert(
                    id.clone(),
                    DisplayInfo {
                        id,
                        ip,
                        port: info.get_port(),
                        hostname: info.get_hostname().trim_end_matches('.').to_owned(),
                        width,
                        height,
                    },
                );
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }

    let _ = mdns.shutdown();

    let mut result: Vec<DisplayInfo> = displays.into_values().collect();
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}

/// Resolve a target string to matching displays.
pub fn resolve_target<'a>(
    displays: &'a [DisplayInfo],
    target: &str,
) -> Result<Vec<&'a DisplayInfo>> {
    if target == "all" {
        if displays.is_empty() {
            return Err(PaintressError::NoDisplaysFound);
        }
        return Ok(displays.iter().collect());
    }

    for d in displays {
        if target == d.id || target == d.hostname || target == d.ip || d.hostname.starts_with(target)
        {
            return Ok(vec![d]);
        }
    }

    Err(PaintressError::DisplayNotFound(target.to_owned()))
}
