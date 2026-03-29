use thiserror::Error;

#[derive(Debug, Error)]
pub enum PaintressError {
    #[error("no displays found on network")]
    NoDisplaysFound,

    #[error("display not found: {0}")]
    DisplayNotFound(String),

    #[error("grid {layout} needs {needed} displays, but only {available} found")]
    NotEnoughDisplays {
        layout: String,
        needed: usize,
        available: usize,
    },

    #[error("invalid layout format '{0}' — expected COLSxROWS (e.g. 2x1)")]
    InvalidLayout(String),

    #[error("invalid rotation '{0}' — expected none, cw90, ccw90, or flip180")]
    InvalidRotation(String),

    #[error("invalid position '{0}' — options: left, right, top, bottom, topleft, topright, bottomleft, bottomright")]
    InvalidPosition(String),

    #[error("invalid placement spec '{0}' — expected DISPLAY@COL,ROW[:ROTATION]")]
    InvalidPlacementSpec(String),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Generic(String),
}

pub type Result<T> = std::result::Result<T, PaintressError>;
