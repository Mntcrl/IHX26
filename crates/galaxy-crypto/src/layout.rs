//! Analysis raster, tiles and Morton-ordered commitments (§6).
pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 192;
pub const CENTRAL_TILES: [u8; 16] = [10, 11, 12, 13, 18, 19, 20, 21, 26, 27, 28, 29, 34, 35, 36, 37];
pub const PERIPHERAL_TILES: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 14, 15, 16, 17, 22, 23, 24, 25, 30, 31, 32, 33, 38, 39,
    40, 41, 42, 43, 44, 45, 46, 47,
];

/// Interleave the low three bits of each coordinate, x first.
pub const fn morton(x: u8, y: u8) -> u8 {
    (x & 1) | ((y & 1) << 1) | ((x & 2) << 1) | ((y & 2) << 2) | ((x & 4) << 2) | ((y & 4) << 3)
}

pub const fn tile_label(tx: u8, ty: u8) -> u8 {
    ty * 8 + tx
}

pub const fn tile_xy(tile: u8) -> (u8, u8) {
    (tile % 8, tile / 8)
}

/// Slot from a row-major tile label and block coordinates within that tile.
/// Labels 48..64 describe padding; bx and by must be in 0..8.
pub const fn leaf_slot(tile: u8, bx: u8, by: u8) -> u16 {
    let (tx, ty) = tile_xy(tile);
    leaf_slot_xy(tx, ty, bx, by)
}

pub const fn leaf_slot_xy(tx: u8, ty: u8, bx: u8, by: u8) -> u16 {
    assert!(tx < 8 && ty < 8 && bx < 8 && by < 8);
    (morton(tx, ty) as u16) * 64 + morton(bx, by) as u16
}
