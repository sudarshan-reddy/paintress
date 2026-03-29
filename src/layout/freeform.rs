use std::collections::BTreeMap;

use crate::discovery::DisplayInfo;
use crate::error::Result;
use crate::image::Rotation;

use super::{Layout, Placement};

/// A freeform layout where each display is placed at an explicit grid position
/// with independent rotation.
///
/// ```ignore
/// let layout = FreeformBuilder::new()
///     .place(&display_a, 0, 0, Rotation::None)
///     .place(&display_b, 1, 0, Rotation::Cw90)
///     .build()?;
/// ```
pub struct FreeformBuilder {
    entries: Vec<FreeformEntry>,
}

struct FreeformEntry {
    display: DisplayInfo,
    col: u32,
    row: u32,
    rotation: Rotation,
}

impl FreeformBuilder {
    pub fn new() -> Self {
        FreeformBuilder {
            entries: Vec::new(),
        }
    }

    /// Place a display at the given grid (col, row) with a rotation.
    pub fn place(
        mut self,
        display: &DisplayInfo,
        col: u32,
        row: u32,
        rotation: Rotation,
    ) -> Self {
        self.entries.push(FreeformEntry {
            display: display.clone(),
            col,
            row,
            rotation,
        });
        self
    }

    pub fn build(self) -> Result<FreeformLayout> {
        // Compute per-column widths and per-row heights from the placed displays.
        let mut col_widths: BTreeMap<u32, u32> = BTreeMap::new();
        let mut row_heights: BTreeMap<u32, u32> = BTreeMap::new();

        for e in &self.entries {
            let (cw, ch) = e.rotation.canvas_dims(e.display.width, e.display.height);
            col_widths
                .entry(e.col)
                .and_modify(|w| *w = (*w).max(cw))
                .or_insert(cw);
            row_heights
                .entry(e.row)
                .and_modify(|h| *h = (*h).max(ch))
                .or_insert(ch);
        }

        // Cumulative offsets
        let mut col_offsets: BTreeMap<u32, u32> = BTreeMap::new();
        let mut x_acc = 0;
        for (&col, &w) in &col_widths {
            col_offsets.insert(col, x_acc);
            x_acc += w;
        }

        let mut row_offsets: BTreeMap<u32, u32> = BTreeMap::new();
        let mut y_acc = 0;
        for (&row, &h) in &row_heights {
            row_offsets.insert(row, y_acc);
            y_acc += h;
        }

        let canvas_w = x_acc;
        let canvas_h = y_acc;

        let placements = self
            .entries
            .into_iter()
            .map(|e| {
                let (cw, ch) = e.rotation.canvas_dims(e.display.width, e.display.height);
                Placement {
                    display: e.display,
                    x: col_offsets[&e.col],
                    y: row_offsets[&e.row],
                    canvas_w: cw,
                    canvas_h: ch,
                    rotation: e.rotation,
                }
            })
            .collect();

        Ok(FreeformLayout {
            canvas_w,
            canvas_h,
            placements,
        })
    }
}

pub struct FreeformLayout {
    canvas_w: u32,
    canvas_h: u32,
    placements: Vec<Placement>,
}

impl Layout for FreeformLayout {
    fn canvas_size(&self) -> (u32, u32) {
        (self.canvas_w, self.canvas_h)
    }

    fn placements(&self) -> &[Placement] {
        &self.placements
    }
}
