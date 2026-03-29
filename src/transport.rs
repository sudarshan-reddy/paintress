use crate::discovery::DisplayInfo;
use crate::error::Result;

/// Fetch `/info` JSON from a display.
pub async fn fetch_info(display: &DisplayInfo) -> Result<serde_json::Value> {
    let url = format!("http://{}:{}/info", display.ip, display.port);
    let resp = reqwest::get(&url).await?;
    let text = resp.text().await?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| crate::error::PaintressError::Generic(e.to_string()))?;
    Ok(json)
}

/// POST raw 4bpp image data to a display's `/display` endpoint.
pub async fn send_raw(display: &DisplayInfo, data: Vec<u8>) -> Result<String> {
    let url = format!("http://{}:{}/display", display.ip, display.port);
    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header("Content-Type", "application/octet-stream")
        .body(data)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    Ok(format!("{}: {status} — {}", display.id, body.trim()))
}
