use crate::discovery::DisplayInfo;
use crate::error::{PaintressError, Result};
use crate::image::Rotation;

use super::{Layout, Placement};

/// Grid cell override for per-cell display and rotation assignment.
struct CellOverride {
    col: u32,
    row: u32,
    display: DisplayInfo,
    rotation: Rotation,
}

/// A regular grid layout that splits an image into COLSxROWS tiles.
///
/// ```ignore
/// let layout = Grid::new(2, 1)
///     .displays(&discovered)
///     .build()?;
///
/// // With per-cell rotation:
/// let layout = Grid::new(2, 1)
///     .cell(0, 0, &display_a, Rotation::None)
///     .cell(1, 0, &display_b, Rotation::Cw90)
///     .build()?;
/// ```
pub struct GridBuilder {
    cols: u32,
    rows: u32,
    auto_displays: Option<Vec<DisplayInfo>>,
    overrides: Vec<CellOverride>,
    default_rotation: Rotation,
}

impl GridBuilder {
    pub fn new(cols: u32, rows: u32) -> Self {
        GridBuilder {
            cols,
            rows,
            auto_displays: None,
            overrides: Vec::new(),
            default_rotation: Rotation::None,
        }
    }

    /// Auto-assign displays in row-major order.
    pub fn displays(mut self, displays: &[DisplayInfo]) -> Self {
        self.auto_displays = Some(displays.to_vec());
        self
    }

    /// Set the default rotation for all cells (overridden by per-cell settings).
    pub fn rotation(mut self, rotation: Rotation) -> Self {
        self.default_rotation = rotation;
        self
    }

    /// Assign a specific display and rotation to a grid cell.
    pub fn cell(mut self, col: u32, row: u32, display: &DisplayInfo, rotation: Rotation) -> Self {
        self.overrides.push(CellOverride {
            col,
            row,
            display: display.clone(),
            rotation,
        });
        self
    }

    pub fn build(self) -> Result<GridLayout> {
        let needed = (self.cols * self.rows) as usize;

        // Build the cell assignments: overrides take priority, auto-fill the rest
        let mut cells: Vec<Option<(DisplayInfo, Rotation)>> = vec![None; needed];

        for ov in &self.overrides {
            let idx = (ov.row * self.cols + ov.col) as usize;
            cells[idx] = Some((ov.display.clone(), ov.rotation));
        }

        if let Some(ref auto) = self.auto_displays {
            if auto.len() < needed {
                return Err(PaintressError::NotEnoughDisplays {
                    layout: format!("{}x{}", self.cols, self.rows),
                    needed,
                    available: auto.len(),
                });
            }
            let mut auto_iter = auto.iter();
            for cell in &mut cells {
                if cell.is_none() {
                    let display = auto_iter.next().unwrap().clone();
                    *cell = Some((display, self.default_rotation));
                }
            }
        }

        // Verify all cells are assigned
        for (i, cell) in cells.iter().enumerate() {
            if cell.is_none() {
                let col = i as u32 % self.cols;
                let row = i as u32 / self.cols;
                return Err(PaintressError::Generic(format!(
                    "grid cell ({col},{row}) has no display assigned"
                )));
            }
        }

        // Compute tile dimensions: all cells use the first display's native size
        // (grids assume uniform displays).
        let (first_display, first_rot) = cells[0].as_ref().unwrap();
        let native_w = first_display.width;
        let native_h = first_display.height;
        let (tile_w, tile_h) = first_rot.canvas_dims(native_w, native_h);

        let canvas_w = tile_w * self.cols;
        let canvas_h = tile_h * self.rows;

        let mut placements = Vec::with_capacity(needed);
        for (i, cell) in cells.into_iter().enumerate() {
            let (display, rotation) = cell.unwrap();
            let col = i as u32 % self.cols;
            let row = i as u32 / self.cols;
            let (cw, ch) = rotation.canvas_dims(display.width, display.height);
            placements.push(Placement {
                display,
                x: col * tile_w,
                y: row * tile_h,
                canvas_w: cw,
                canvas_h: ch,
                rotation,
            });
        }

        Ok(GridLayout {
            canvas_w,
            canvas_h,
            placements,
        })
    }
}

pub struct GridLayout {
    canvas_w: u32,
    canvas_h: u32,
    placements: Vec<Placement>,
}

impl Layout for GridLayout {
    fn canvas_size(&self) -> (u32, u32) {
        (self.canvas_w, self.canvas_h)
    }

    fn placements(&self) -> &[Placement] {
        &self.placements
    }
}
