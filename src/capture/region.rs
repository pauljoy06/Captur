use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PixelRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl PixelRect {
    pub fn from_points(a: PixelPoint, b: PixelPoint) -> Self {
        Self {
            left: a.x.min(b.x),
            top: a.y.min(b.y),
            right: a.x.max(b.x),
            bottom: a.y.max(b.y),
        }
    }

    pub const fn width(self) -> u32 {
        self.right.saturating_sub(self.left) as u32
    }

    pub const fn height(self) -> u32 {
        self.bottom.saturating_sub(self.top) as u32
    }

    pub const fn is_empty(self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }

    pub fn clamp_to(self, bounds: Self) -> Self {
        Self {
            left: self.left.clamp(bounds.left, bounds.right),
            top: self.top.clamp(bounds.top, bounds.bottom),
            right: self.right.clamp(bounds.left, bounds.right),
            bottom: self.bottom.clamp(bounds.top, bounds.bottom),
        }
    }
}
