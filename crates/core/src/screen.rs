use crate::id::ScreenId;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeSide {
    Top,
    Bottom,
    Left,
    Right,
}

impl EdgeSide {
    pub fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// Description of one logical screen in a multi-machine layout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenInfo {
    pub id: ScreenId,
    pub name: String,
    /// Total logical width (sum of all monitors on this machine).
    pub width: i32,
    pub height: i32,
}

/// Defines how screens are arranged in 2D. A simple representation: each
/// screen names a neighbour on each side. Cursor crossing an edge with no
/// neighbour is clamped.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScreenLayout {
    pub screens: Vec<ScreenInfo>,
    pub neighbours: Vec<Neighbour>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Neighbour {
    pub from: ScreenId,
    pub side: EdgeSide,
    pub to: ScreenId,
}

impl ScreenLayout {
    pub fn neighbour_of(&self, from: ScreenId, side: EdgeSide) -> Option<ScreenId> {
        self.neighbours
            .iter()
            .find(|n| n.from == from && n.side == side)
            .map(|n| n.to)
    }
}
