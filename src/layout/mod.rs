pub mod freeform;

use crate::discovery::DisplayInfo;
use crate::image::Rotation;

/// A single tile placement on the virtual canvas.
#[derive(Clone, Debug)]
pub struct Placement {
    /// Which display to send this tile to.
    pub display: DisplayInfo,
    /// Pixel offset on the virtual canvas.
    pub x: u32,
    pub y: u32,
    /// Tile size on the canvas (before rotation for the display).
    pub canvas_w: u32,
    pub canvas_h: u32,
    /// Rotation to apply when packing for this display.
    pub rotation: Rotation,
}

/// A layout describes how an image is split across one or more displays.
pub trait Layout {
    /// Total virtual canvas size that the source image should be dithered to.
    fn canvas_size(&self) -> (u32, u32);

    /// The individual tile placements.
    fn placements(&self) -> &[Placement];
}
