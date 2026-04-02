use std::collections::HashMap;
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};

use crate::discovery::DisplayInfo;
use crate::error::{PaintressError, Result};
use crate::image::{DISPLAY_HEIGHT, DISPLAY_WIDTH};

use super::DisplayBackend;

/// ESP32-based backend: mDNS discovery, HTTP transport.
pub struct Esp32Backend;

impl Esp32Backend {
    pub fn new() -> Self {
        Esp32Backend
    }
}

impl DisplayBackend for Esp32Backend {
    async fn discover(&self, timeout: Duration) -> Result<Vec<DisplayInfo>> {
        // mDNS recv_timeout is blocking — run on the blocking threadpool.
        tokio::task::spawn_blocking(move || discover_mdns(timeout))
            .await
            .map_err(|e| PaintressError::Generic(format!("spawn_blocking: {e}")))?
    }

    fn resolve_target<'a>(
        &self,
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
            if target == d.id
                || target == d.hostname
                || target == d.ip
                || d.hostname.starts_with(target)
            {
                return Ok(vec![d]);
            }
        }

        Err(PaintressError::DisplayNotFound(target.to_owned()))
    }

    async fn fetch_info(&self, display: &DisplayInfo) -> Result<serde_json::Value> {
        let url = format!("http://{}:{}/info", display.ip, display.port);
        let resp = reqwest::get(&url).await?;
        let text = resp.text().await?;
        let json: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| PaintressError::Generic(e.to_string()))?;
        Ok(json)
    }

    async fn send_raw(&self, display: &DisplayInfo, data: Vec<u8>) -> Result<String> {
        let url = format!("http://{}:{}/display", display.ip, display.port);
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .timeout(Duration::from_secs(30))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok(format!("{}: {status} — {}", display.id, body.trim()))
    }

    async fn fetch_logs(&self, display: &DisplayInfo) -> Result<String> {
        let url = format!("http://{}:{}/logs?clear=1", display.ip, display.port);
        let resp = reqwest::get(&url)
            .await
            .map_err(|e| PaintressError::Generic(format!("{}: {e}", display.id)))?;
        let text = resp.text().await.unwrap_or_default();
        Ok(text)
    }

    async fn update_firmware(&self, display: &DisplayInfo, firmware: &PathBuf) -> Result<String> {
        let data = std::fs::read(firmware)
            .map_err(|e| PaintressError::Generic(format!("read firmware: {e}")))?;
        let size = data.len();
        let url = format!("http://{}:{}/ota", display.ip, display.port);
        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .timeout(Duration::from_secs(120))
            .send()
            .await?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        Ok(format!(
            "{}: {status} — {} ({size} bytes)",
            display.id,
            body.trim()
        ))
    }
}

/// Blocking mDNS discovery (runs on the blocking threadpool).
fn discover_mdns(timeout: Duration) -> Result<Vec<DisplayInfo>> {
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
                    .unwrap_or_else(|| {
                        info.get_fullname().split('.').next().unwrap_or("unknown")
                    })
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

    // Verify each display is actually reachable (mDNS cache can be stale)
    let mut result: Vec<DisplayInfo> = Vec::new();
    for d in displays.into_values() {
        if let Ok(addr) = format!("{}:{}", d.ip, d.port).parse::<SocketAddr>() {
            if TcpStream::connect_timeout(&addr, Duration::from_secs(1)).is_ok() {
                result.push(d);
            } else {
                eprintln!("  {} ({}) — not reachable, skipping", d.id, d.ip);
            }
        }
    }
    result.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(result)
}
