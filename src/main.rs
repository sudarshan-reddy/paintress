mod backend;
mod config;
mod discovery;
mod error;
mod image;
mod layout;
mod palette;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};

use crate::backend::esp32::Esp32Backend;
use crate::backend::DisplayBackend;
use crate::config::{Config, Mounting};
use crate::discovery::DisplayInfo;
use crate::error::{PaintressError, Result};
use crate::image::{IndexedImage, DISPLAY_HEIGHT, DISPLAY_WIDTH};
use crate::layout::freeform::FreeformBuilder;
use crate::layout::{Layout, Placement};

#[derive(Parser)]
#[command(name = "paintress", about = "E-Ink Display Fleet Orchestrator")]
struct Cli {
    /// mDNS discovery timeout in seconds
    #[arg(long, default_value = "3.0")]
    timeout: f64,

    /// Color saturation boost (1.0 = no boost)
    #[arg(long, default_value = "1.5")]
    saturation: f32,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover displays and generate/update paintress.toml
    Discover,

    /// Show status of all displays
    Status,

    /// Send an image to your display wall (uses paintress.toml)
    Send {
        /// Image file to send
        image: PathBuf,

        /// Save preview instead of sending
        #[arg(long)]
        preview: bool,

        /// After sending, put displays to sleep for N seconds
        #[arg(long)]
        sleep: Option<u32>,
    },

    /// Preview an image without sending (no network needed)
    Preview {
        /// Image file to preview
        image: PathBuf,
    },

    /// Fetch device logs from all displays
    Logs {
        /// Poll continuously every 2 seconds
        #[arg(long)]
        follow: bool,
    },

    /// OTA firmware update via HTTP
    Ota {
        /// Firmware binary file (.bin)
        firmware: PathBuf,

        /// Display ID, hostname, or 'all'
        #[arg(long, default_value = "all")]
        to: String,
    },
}

/// Load or auto-create the config, merging any newly discovered displays.
fn load_or_create_config(displays: &[DisplayInfo]) -> Result<Config> {
    let config = match Config::load()? {
        Some(mut cfg) => {
            cfg.merge_discovered(displays);
            cfg.save()?;
            cfg
        }
        None => {
            let cfg = Config::from_discovered(displays);
            cfg.save()?;
            cfg
        }
    };
    Ok(config)
}

async fn cmd_discover(backend: &impl DisplayBackend, timeout: f64) -> Result<()> {
    let displays = backend.discover(std::time::Duration::from_secs_f64(timeout)).await?;
    if displays.is_empty() {
        eprintln!("No displays found.");
        return Ok(());
    }

    let config = load_or_create_config(&displays)?;

    eprintln!("\nFound {} display(s):\n", displays.len());
    for d in &displays {
        let cfg = config.display.iter().find(|c| c.serial == d.id);
        let name = cfg.map(|c| c.name.as_str()).unwrap_or(&d.id);
        let mounted = cfg.map(|c| c.mounted).unwrap_or_default();
        let col = cfg.map(|c| c.col).unwrap_or(0);
        let row = cfg.map(|c| c.row).unwrap_or(0);

        println!("  {} ({})", name, d.id);
        println!("    IP:       {}:{}", d.ip, d.port);
        println!("    Hostname: {}", d.hostname);
        println!("    Size:     {}x{}", d.width, d.height);
        println!("    Position: col={col}, row={row}");
        println!("    Mounted:  {mounted:?}");
        println!();
    }

    eprintln!("Config written to paintress.toml");
    Ok(())
}

async fn cmd_status<B: DisplayBackend>(backend: &Arc<B>, timeout: f64) -> Result<()> {
    let displays = backend.discover(std::time::Duration::from_secs_f64(timeout)).await?;
    if displays.is_empty() {
        eprintln!("No displays found.");
        return Ok(());
    }
    eprintln!("\nQuerying {} display(s)...\n", displays.len());

    let mut handles = Vec::new();
    for d in displays {
        let backend = Arc::clone(backend);
        handles.push(tokio::spawn(async move {
            match backend.fetch_info(&d).await {
                Ok(info) => {
                    let id = info
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&d.id);
                    let status = info
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let uptime = info
                        .get("uptime")
                        .and_then(|v| v.as_u64())
                        .map(|v| format!("{v}s"))
                        .unwrap_or_else(|| "?".into());
                    let ip = info
                        .get("ip")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&d.ip);
                    format!("  {id}: {status}  uptime={uptime}  ip={ip}")
                }
                Err(e) => format!("  {}: error — {e}", d.id),
            }
        }));
    }

    for h in handles {
        if let Ok(line) = h.await {
            println!("{line}");
        }
    }
    println!();
    Ok(())
}

/// Build a layout from the config file.
fn build_layout_from_config(
    config: &Config,
    discovered: &[DisplayInfo],
) -> Result<(Box<dyn Layout>, Vec<(DisplayInfo, Mounting)>)> {
    let resolved = config.resolve(discovered)?;

    let mut builder = FreeformBuilder::new();
    let mut display_info = Vec::new();

    for (dc, di) in &resolved {
        let rotation = dc.mounted.rotation();
        builder = builder.place(di, dc.col, dc.row, rotation);
        display_info.push(((*di).clone(), dc.mounted));
    }

    let layout = builder.build()?;
    Ok((Box::new(layout), display_info))
}

async fn cmd_send<B: DisplayBackend>(
    backend: &Arc<B>,
    image_path: &PathBuf,
    preview_only: bool,
    sleep_seconds: Option<u32>,
    timeout: f64,
    saturation: f32,
) -> Result<()> {
    let config = Config::load()?;
    let expected_count = config.as_ref().map(|c| c.display.len()).unwrap_or(0);

    // Discovery with retry: if we find fewer displays than config expects,
    // retry at 10s intervals (displays may still be waking from sleep).
    let displays = discover_with_retry(backend, timeout, expected_count).await?;
    if displays.is_empty() {
        return Err(PaintressError::NoDisplaysFound);
    }

    let config = load_or_create_config(&displays)?;
    let (layout, _display_info) = build_layout_from_config(&config, &displays)?;
    let (canvas_w, canvas_h) = layout.canvas_size();

    eprintln!(
        "Canvas: {canvas_w}x{canvas_h} across {} display(s)",
        layout.placements().len()
    );
    for p in layout.placements() {
        let mounting = config
            .display
            .iter()
            .find(|c| c.serial == p.display.id)
            .map(|c| format!("{:?}", c.mounted))
            .unwrap_or_else(|| "landscape".into());
        let name = config
            .display
            .iter()
            .find(|c| c.serial == p.display.id)
            .map(|c| c.name.as_str())
            .unwrap_or(&p.display.id);
        eprintln!(
            "  {name:>16}  ({},{})  {}x{}  {mounting}",
            p.x, p.y, p.canvas_w, p.canvas_h
        );
    }

    let indexed = IndexedImage::from_file(image_path, canvas_w, canvas_h, saturation)?;

    if preview_only {
        save_preview(&indexed, layout.placements(), &config)?;
        return Ok(());
    }

    send_tiles(backend, &indexed, layout.placements()).await?;

    // After sending, optionally put displays to sleep
    if let Some(seconds) = sleep_seconds {
        let all_displays: Vec<DisplayInfo> = layout.placements().iter().map(|p| p.display.clone()).collect();
        wait_for_ready(backend, &all_displays).await;
        sleep_displays(backend, &all_displays, seconds).await;
    }

    Ok(())
}

fn cmd_preview(image_path: &PathBuf, saturation: f32) -> Result<()> {
    let config = Config::load()?;

    let (canvas_w, canvas_h, placements) = if let Some(ref config) = config {
        let mut builder = FreeformBuilder::new();
        for dc in &config.display {
            let (_cw, _ch) = dc.mounted.canvas_dims(DISPLAY_WIDTH, DISPLAY_HEIGHT);
            let dummy = DisplayInfo {
                id: dc.serial.clone(),
                ip: String::new(),
                port: 0,
                hostname: String::new(),
                width: DISPLAY_WIDTH,
                height: DISPLAY_HEIGHT,
            };
            builder = builder.place(&dummy, dc.col, dc.row, dc.mounted.rotation());
        }
        let layout = builder.build()?;
        let (w, h) = layout.canvas_size();
        let p = layout.placements().to_vec();
        (w, h, p)
    } else {
        (DISPLAY_WIDTH, DISPLAY_HEIGHT, Vec::new())
    };

    eprintln!("Preview: {canvas_w}x{canvas_h}");

    let indexed = IndexedImage::from_file(image_path, canvas_w, canvas_h, saturation)?;

    if !placements.is_empty() {
        save_preview(&indexed, &placements, config.as_ref().unwrap())?;
    } else {
        let preview = indexed.to_rgb();
        preview.save("preview.png")?;
        eprintln!("Saved preview to preview.png");
    }

    Ok(())
}

fn save_preview(indexed: &IndexedImage, placements: &[Placement], _config: &Config) -> Result<()> {
    let mut preview = indexed.to_rgb();

    let magenta = ::image::Rgb([255u8, 0, 255]);
    for p in placements {
        let x0 = p.x;
        let y0 = p.y;
        let x1 = p.x + p.canvas_w;
        let y1 = p.y + p.canvas_h;

        for x in x0..x1 {
            if y0 > 0 {
                preview.put_pixel(x, y0, magenta);
            }
            if y1 < indexed.height {
                preview.put_pixel(x, y1 - 1, magenta);
            }
        }
        for y in y0..y1 {
            if x0 > 0 {
                preview.put_pixel(x0, y, magenta);
            }
            if x1 < indexed.width {
                preview.put_pixel(x1 - 1, y, magenta);
            }
        }
    }

    preview.save("preview.png")?;
    eprintln!("Saved preview to preview.png");
    Ok(())
}

async fn send_tiles<B: DisplayBackend>(
    backend: &Arc<B>,
    indexed: &IndexedImage,
    placements: &[Placement],
) -> Result<()> {
    eprintln!("Sending tiles...");

    let mut handles = Vec::new();
    for p in placements {
        let tile = indexed.crop(p.x, p.y, p.canvas_w, p.canvas_h);
        let raw = tile.pack_rotated(p.rotation);
        let display = p.display.clone();
        let backend = Arc::clone(backend);
        handles.push(tokio::spawn(async move {
            backend.send_raw(&display, raw).await
        }));
    }

    for h in handles {
        match h.await {
            Ok(Ok(msg)) => println!("  {msg}"),
            Ok(Err(e)) => println!("  error: {e:?}"),
            Err(e) => println!("  task error: {e}"),
        }
    }
    Ok(())
}

/// Discovery with retry: waits for sleeping displays to come back online.
/// Retries at 10s intervals for up to 5 attempts if fewer displays found than expected.
async fn discover_with_retry<B: DisplayBackend>(
    backend: &Arc<B>,
    timeout: f64,
    expected_count: usize,
) -> Result<Vec<DisplayInfo>> {
    let discovery_timeout = std::time::Duration::from_secs_f64(timeout);
    let max_retries = if expected_count > 0 { 5 } else { 0 };

    for attempt in 0..=max_retries {
        let displays = backend.discover(discovery_timeout).await?;

        if expected_count == 0 || displays.len() >= expected_count {
            return Ok(displays);
        }

        if attempt < max_retries {
            eprintln!(
                "Found {}/{} displays, retrying in 10s (displays may be waking from sleep)...",
                displays.len(),
                expected_count
            );
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        } else {
            // Last attempt — return whatever we found
            eprintln!(
                "Found {}/{} displays after {} retries, proceeding",
                displays.len(),
                expected_count,
                max_retries
            );
            return Ok(displays);
        }
    }

    unreachable!()
}

/// Poll displays until all report status "ready" (refresh complete).
/// Times out after 90s per display.
async fn wait_for_ready<B: DisplayBackend>(backend: &Arc<B>, displays: &[DisplayInfo]) {
    eprintln!("Waiting for displays to finish refreshing...");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(90);

    let mut pending: Vec<&DisplayInfo> = displays.iter().collect();
    while !pending.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let mut still_pending = Vec::new();
        for d in &pending {
            match backend.fetch_info(d).await {
                Ok(info) => {
                    let status = info
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    if status != "ready" {
                        still_pending.push(*d);
                    }
                }
                Err(_) => {
                    still_pending.push(*d);
                }
            }
        }
        pending = still_pending;
    }
    if !pending.is_empty() {
        eprintln!(
            "Warning: {} display(s) still not ready after 90s, sending sleep anyway",
            pending.len()
        );
    }
}

/// Send sleep command to all displays concurrently.
async fn sleep_displays<B: DisplayBackend>(
    backend: &Arc<B>,
    displays: &[DisplayInfo],
    seconds: u32,
) {
    eprintln!("Sending sleep command ({}s) to {} display(s)...", seconds, displays.len());
    let mut handles = Vec::new();
    for d in displays {
        let display = d.clone();
        let backend = Arc::clone(backend);
        handles.push(tokio::spawn(async move {
            backend.send_sleep(&display, seconds).await
        }));
    }
    for h in handles {
        match h.await {
            Ok(Ok(msg)) => eprintln!("  {msg}"),
            Ok(Err(e)) => eprintln!("  sleep error: {e}"),
            Err(e) => eprintln!("  task error: {e}"),
        }
    }
}

async fn cmd_logs<B: DisplayBackend>(
    backend: &Arc<B>,
    follow: bool,
    timeout: f64,
) -> Result<()> {
    loop {
        let displays = backend
            .discover(std::time::Duration::from_secs_f64(timeout))
            .await?;
        if displays.is_empty() {
            eprintln!("No displays found.");
            if !follow {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        }

        let mut handles = Vec::new();
        for d in displays {
            let backend = Arc::clone(backend);
            handles.push(tokio::spawn(async move {
                let id = d.id.clone();
                match backend.fetch_logs(&d).await {
                    Ok(text) => (id, text),
                    Err(e) => (id, format!("error: {e}")),
                }
            }));
        }

        for h in handles {
            if let Ok((id, text)) = h.await {
                let text = text.trim();
                if !text.is_empty() {
                    for line in text.lines() {
                        println!("[{id}] {line}");
                    }
                }
            }
        }

        if !follow {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Ok(())
}

async fn cmd_ota<B: DisplayBackend>(
    backend: &Arc<B>,
    firmware: &PathBuf,
    to: &str,
    timeout: f64,
) -> Result<()> {
    if !firmware.exists() {
        return Err(PaintressError::Generic(format!(
            "firmware file not found: {}",
            firmware.display()
        )));
    }

    let displays = backend
        .discover(std::time::Duration::from_secs_f64(timeout))
        .await?;
    let targets = backend.resolve_target(&displays, to)?;

    eprintln!(
        "OTA update: {} -> {} display(s)\n",
        firmware.display(),
        targets.len()
    );

    for t in targets {
        eprintln!("  Updating {} ({})...", t.id, t.hostname);
        match backend.update_firmware(t, firmware).await {
            Ok(msg) => println!("    {msg}"),
            Err(e) => println!("    {}: error — {e}", t.id),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let backend = Arc::new(Esp32Backend::new());

    let result = match cli.command {
        Command::Discover => cmd_discover(&*backend, cli.timeout).await,
        Command::Status => cmd_status(&backend, cli.timeout).await,
        Command::Send {
            ref image,
            preview,
            sleep,
        } => cmd_send(&backend, image, preview, sleep, cli.timeout, cli.saturation).await,
        Command::Preview { ref image } => cmd_preview(image, cli.saturation),
        Command::Logs { follow } => cmd_logs(&backend, follow, cli.timeout).await,
        Command::Ota {
            ref firmware,
            ref to,
        } => cmd_ota(&backend, firmware, to, cli.timeout).await,
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
