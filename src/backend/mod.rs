pub mod esp32;

use std::path::PathBuf;
use std::time::Duration;

use crate::discovery::DisplayInfo;
use crate::error::Result;

/// Core abstraction over a display fleet's network protocol.
///
/// A backend knows how to discover displays, query their status, and send
/// image data. Different hardware (ESP32 over HTTP, Pico over USB, a local
/// simulator, etc.) each get their own implementation.
///
/// All methods are async so implementations can use non-blocking I/O or
/// offload blocking work to `spawn_blocking` without stalling the runtime.
pub trait DisplayBackend: Send + Sync + 'static {
    /// Scan the network/bus for available displays.
    fn discover(
        &self,
        timeout: Duration,
    ) -> impl std::future::Future<Output = Result<Vec<DisplayInfo>>> + Send;

    /// Resolve a user-supplied target string ("all", a hostname, an ID, etc.)
    /// to matching displays from a previously discovered set.
    ///
    /// This is pure logic — no I/O — but lives on the trait so backends can
    /// customize matching (e.g. USB serial numbers vs mDNS hostnames).
    fn resolve_target<'a>(
        &self,
        displays: &'a [DisplayInfo],
        target: &str,
    ) -> Result<Vec<&'a DisplayInfo>>;

    /// Fetch status/info JSON from a single display.
    fn fetch_info(
        &self,
        display: &DisplayInfo,
    ) -> impl std::future::Future<Output = Result<serde_json::Value>> + Send;

    /// Send packed image data to a single display.
    fn send_raw(
        &self,
        display: &DisplayInfo,
        data: Vec<u8>,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Fetch device logs from a single display (clears the buffer after read).
    fn fetch_logs(
        &self,
        display: &DisplayInfo,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    /// Send a firmware binary to a single display for OTA update.
    fn update_firmware(
        &self,
        display: &DisplayInfo,
        firmware: &PathBuf,
    ) -> impl std::future::Future<Output = Result<String>> + Send;
}
