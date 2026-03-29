use std::path::Path;

use crate::discovery::DisplayInfo;
use crate::error::Result;

/// OTA firmware update via espota.py.
pub fn ota_update(display: &DisplayInfo, firmware: &Path) -> Result<String> {
    // Try espota.py first (Arduino IDE tool)
    let result = std::process::Command::new("espota.py")
        .args(["-i", &display.hostname, "-p", "3232", "-f"])
        .arg(firmware)
        .output();

    match result {
        Ok(output) if output.status.success() => {
            Ok(format!("{}: OK", display.id))
        }
        _ => {
            // Fallback: esptool via python module
            let result = std::process::Command::new("python3")
                .args([
                    "-m",
                    "esptool",
                    "--chip",
                    "esp32s3",
                    "--port",
                    &display.hostname,
                    "write_flash",
                    "0x10000",
                ])
                .arg(firmware)
                .output()?;

            if result.status.success() {
                Ok(format!("{}: OK", display.id))
            } else {
                let err = String::from_utf8_lossy(&result.stderr);
                let out = String::from_utf8_lossy(&result.stdout);
                Ok(format!(
                    "{}: FAILED — {}",
                    display.id,
                    if err.is_empty() {
                        out.trim().to_string()
                    } else {
                        err.trim().to_string()
                    }
                ))
            }
        }
    }
}
