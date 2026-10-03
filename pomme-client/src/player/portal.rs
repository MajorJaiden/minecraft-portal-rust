//! A deliberately local-only first Portal prototype.
//!
//! It owns the temporary glass blocks used as a visible portal marker.  A
//! later rendering pass can replace those markers without changing placement
//! or traversal behaviour.

use azalea_block::BlockState;
use azalea_core::direction::Direction;
use azalea_core::position::BlockPos;
use glam::DVec3;

use crate::entity::components::Position;
use crate::player::LocalPlayer;
use crate::player::interaction::BlockHitResult;
use crate::world::block::{find_state, is_air};
use crate::world::chunk::ChunkStore;

const PORTAL_WIDTH: i32 = 2;
const PORTAL_HEIGHT: i32 = 3;
const EXIT_DISTANCE: f64 = 1.15;
const ENTRY_DISTANCE: f64 = 0.85;
const TELEPORT_COOLDOWN_TICKS: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortalColor {
    Blue,
    Orange,
}

impl PortalColor {
    fn marker_block(self) -> BlockState {
        find_state(
            match self {
                Self::Blue => "blue_stained_glass",
                Self::Orange => "orange_stained_glass",
            },
            &[],
        )
    }
}

#[derive(Clone, Copy)]
struct PortalBlock {
    pos: BlockPos,
    replaced: BlockState,
}

#[derive(Clone)]
struct Portal {
    normal: Direction,
    blocks: Vec<PortalBlock>,
}

impl Portal {
    fn entrance_center(&self) -> DVec3 {
        // The portal list starts at its bottom-left corner.  Averaging the
        // marker cells gives a robust entrance center for either orientation.
        let sum = self.blocks.iter().fold(DVec3::ZERO, |sum, block| {
            sum + DVec3::new(
                f64::from(block.pos.x) + 0.5,
                f64::from(block.pos.y) + 0.5,
                f64::from(block.pos.z) + 0.5,
            )
        });
        sum / self.blocks.len() as f64
    }

    fn exit_position(&self) -> Position {
        let center = self.entrance_center();
        let normal = self.normal.normal();
        Position::new(
            center.x + f64::from(normal.x) * EXIT_DISTANCE,
            self.blocks[0].pos.y as f64 + 0.05,
            center.z + f64::from(normal.z) * EXIT_DISTANCE,
        )
    }

    fn contains(&self, position: Position) -> bool {
        let center = self.entrance_center();
        let normal = self.normal.normal();
        let (nx, nz) = (normal.x, normal.z);
        let normal_distance =
            (position.x - center.x) * f64::from(nx) + (position.z - center.z) * f64::from(nz);
        let lateral_distance = if nx == 0 {
            (position.x - center.x).abs()
        } else {
            (position.z - center.z).abs()
        };
        normal_distance.abs() <= ENTRY_DISTANCE
            && lateral_distance <= f64::from(PORTAL_WIDTH) / 2.0
            && position.y >= self.blocks[0].pos.y as f64 - 0.1
            && position.y <= self.blocks[0].pos.y as f64 + f64::from(PORTAL_HEIGHT)
    }
}

#[derive(Default)]
pub struct PortalState {
    blue: Option<Portal>,
    orange: Option<Portal>,
    cooldown: u8,
}

impl PortalState {
    /// Place a 2x3 portal on a horizontal block face. Returns every changed
    /// marker cell so the caller can light and re-mesh them through the normal
    /// client-side edit path.
    pub fn place(
        &mut self,
        color: PortalColor,
        hit: BlockHitResult,
        chunks: &ChunkStore,
    ) -> Result<Vec<BlockPos>, &'static str> {
        let Some(cells) = portal_cells(hit.block_pos, hit.face) else {
            return Err("Portals must be placed on a wall");
        };
        if !cells
            .iter()
            .all(|pos| is_air(chunks.get_block_state(pos.x, pos.y, pos.z)))
        {
            return Err("Portal needs a clear 2x3 space");
        }

        let mut changed = self.clear(color, chunks);
        let marker = color.marker_block();
        let blocks = cells
            .into_iter()
            .map(|pos| {
                let replaced = chunks.get_block_state(pos.x, pos.y, pos.z);
                chunks.set_block_state(pos.x, pos.y, pos.z, marker);
                PortalBlock { pos, replaced }
            })
            .collect::<Vec<_>>();
        changed.extend(blocks.iter().map(|block| block.pos));
        let portal = Portal {
            normal: hit.face,
            blocks,
        };
        match color {
            PortalColor::Blue => self.blue = Some(portal),
            PortalColor::Orange => self.orange = Some(portal),
        }
        Ok(changed)
    }

    pub fn tick_teleport(&mut self, player: &mut LocalPlayer) -> bool {
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return false;
        }
        let destination = match (&self.blue, &self.orange) {
            (Some(blue), Some(orange)) if blue.contains(player.position) => orange.exit_position(),
            (Some(blue), Some(orange)) if orange.contains(player.position) => blue.exit_position(),
            _ => return false,
        };
        player.position = destination;
        player.prev_position = destination;
        player.velocity = Default::default();
        self.cooldown = TELEPORT_COOLDOWN_TICKS;
        true
    }

    fn clear(&mut self, color: PortalColor, chunks: &ChunkStore) -> Vec<BlockPos> {
        let old = match color {
            PortalColor::Blue => self.blue.take(),
            PortalColor::Orange => self.orange.take(),
        };
        if let Some(portal) = old {
            let mut changed = Vec::with_capacity(portal.blocks.len());
            for block in portal.blocks {
                chunks.set_block_state(block.pos.x, block.pos.y, block.pos.z, block.replaced);
                changed.push(block.pos);
            }
            changed
        } else {
            Vec::new()
        }
    }
}

fn portal_cells(anchor: BlockPos, face: Direction) -> Option<Vec<BlockPos>> {
    let normal = face.normal();
    let (nx, ny, nz) = (normal.x, normal.y, normal.z);
    if ny != 0 {
        return None;
    }
    let (side_x, side_z) = if nx == 0 { (1, 0) } else { (0, 1) };
    let mut cells = Vec::with_capacity((PORTAL_WIDTH * PORTAL_HEIGHT) as usize);
    for y in 0..PORTAL_HEIGHT {
        for side in 0..PORTAL_WIDTH {
            cells.push(BlockPos::new(
                anchor.x + nx + side_x * side,
                anchor.y + y,
                anchor.z + nz + side_z * side,
            ));
        }
    }
    Some(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wall_portal_is_two_by_three_in_front_of_target_face() {
        let cells = portal_cells(BlockPos::new(10, 64, 10), Direction::North).unwrap();
        assert_eq!(cells.len(), 6);
        assert_eq!(cells[0], BlockPos::new(10, 64, 9));
        assert_eq!(cells[5], BlockPos::new(11, 66, 9));
    }

    #[test]
    fn floor_and_ceiling_faces_are_rejected() {
        assert!(portal_cells(BlockPos::new(0, 0, 0), Direction::Up).is_none());
        assert!(portal_cells(BlockPos::new(0, 0, 0), Direction::Down).is_none());
    }
}
