use thiserror::Error;

#[derive(Debug, Error)]
pub enum PaintressError {
    #[error("no displays found on network")]
    NoDisplaysFound,

    #[error("display not found: {0}")]
    DisplayNotFound(String),

    #[error("invalid rotation '{0}' — expected none, cw90, ccw90, or flip180")]
    InvalidRotation(String),

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
