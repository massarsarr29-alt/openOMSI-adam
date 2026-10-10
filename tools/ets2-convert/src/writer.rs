//! Writes an OMSI 2 map folder: `global.cfg`, `tile_x_y.map` with its `.terrain`, and
//! `ailists.cfg`, in the layout `docs/FORMATS.md` describes and `omsi-map` reads back.

use anyhow::{Context, Result};
use omsi_map::terrain::Terrain;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

/// A spline record of a tile, in the tile's own frame: x east, y north, z up (metres).
#[derive(Debug, Clone)]
pub struct Spline {
    /// Spline type relative to the OMSI root, e.g. `Splines\Marcel\str_typ001.sli`.
    pub file: String,
    pub id: i64,
    /// Neighbour IDs; 0 = none.
    pub prev_id: i64,
    pub next_id: i64,
    pub pos: [f64; 3],
    /// Degrees, 0 = +y, clockwise positive.
    pub heading: f64,
    pub length: f64,
    /// 0 = straight; > 0 turns right, < 0 left.
    pub radius: f64,
    /// Percent.
    pub grad_start: f64,
    pub grad_end: f64,
    /// Where the spline's textures start along its chain (the chain's length before it).
    pub tex_offset: f64,
}

impl Spline {
    /// Where the spline ends and which way it points there.
    pub fn end(&self) -> ([f64; 2], f64) {
        let h = self.heading.to_radians();
        let (x, y) = (self.pos[0], self.pos[1]);
        if self.radius == 0.0 {
            return (
                [x + self.length * h.sin(), y + self.length * h.cos()],
                self.heading,
            );
        }
        // the centre lies to the right of the direction for a right turn, to the left otherwise
        let r = self.radius;
        let turn = self.length / r;
        let (cx, cy) = (x + r * h.cos(), y - r * h.sin());
        let h2 = h + turn;
        (
            [cx - r * h2.cos(), cy + r * h2.sin()],
            self.heading + turn.to_degrees(),
        )
    }
}

#[derive(Debug, Clone, Default)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
    pub splines: Vec<Spline>,
    pub terrain: Option<Terrain>,
}

impl Tile {
    pub fn file_name(&self) -> String {
        format!("tile_{}_{}.map", self.x, self.y)
    }
}

/// Where the player's bus starts: a place on a tile, without an object behind it.
#[derive(Debug, Clone)]
pub struct EntryPoint {
    pub name: String,
    /// Index of the tile in `Map::tiles`.
    pub tile: usize,
    pub pos: [f64; 3],
    pub heading: f64,
}

/// A weighted list of AI vehicles (`[aigroup_2]`).
#[derive(Debug, Clone)]
pub struct AiGroup {
    pub name: String,
    pub vehicles: Vec<(String, u32)>,
}

#[derive(Debug, Clone, Default)]
pub struct Map {
    pub name: String,
    pub friendly_name: String,
    pub description: String,
    /// Ground texture and its detail texture, relative to the map folder.
    pub ground_texture: (String, String),
    pub tiles: Vec<Tile>,
    pub entry_points: Vec<EntryPoint>,
    pub ai_groups: Vec<AiGroup>,
}

/// Map files are UTF-16 LE with a byte order mark and CR LF, like the OMSI editor writes them.
fn utf16(text: &str) -> Vec<u8> {
    let mut out = vec![0xFF, 0xFE];
    for u in text
        .replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .encode_utf16()
    {
        out.extend_from_slice(&u.to_le_bytes());
    }
    out
}

/// Other text files are Windows-1252 with CR LF.
fn ansi(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .chars()
        .map(|c| if (c as u32) < 0x80 { c as u8 } else { b'?' })
        .collect()
}

impl Map {
    fn next_id(&self) -> i64 {
        self.tiles
            .iter()
            .flat_map(|t| t.splines.iter().map(|s| s.id))
            .max()
            .unwrap_or(0)
            + 1
    }

    pub fn global_cfg(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "File created with ets2-convert\n");
        let _ = writeln!(s, "[name]\n{}\n", self.name);
        let _ = writeln!(s, "[friendlyname]\n{}\n", self.friendly_name);
        let _ = writeln!(s, "[description]\n{}\n[end]\n", self.description);
        let _ = writeln!(s, "[version]\n14\n");
        let _ = writeln!(s, "[NextIDCode]\n{}\n", self.next_id());
        let _ = writeln!(s, "[moneysystem]\nMoney\\DM\\DM.cfg\n");
        let _ = writeln!(s, "[years]\n2000\n2030\n");
        let _ = writeln!(
            s,
            "[groundtex]\n{}\n{}\n0\n1\n60\n",
            self.ground_texture.0, self.ground_texture.1
        );
        if !self.entry_points.is_empty() {
            let _ = writeln!(s, "[entrypoints]\n{}", self.entry_points.len());
            for (i, e) in self.entry_points.iter().enumerate() {
                // a rotation about the vertical axis as a quaternion (x, y, z, w), y being up
                let half = e.heading.to_radians() / 2.0;
                let _ = writeln!(
                    s,
                    "{i}\n0\n0\n{:.3}\n{:.3}\n{:.3}\n0.000\n{:.6}\n0.000\n{:.6}\n{}\n{}",
                    e.pos[0],
                    e.pos[2],
                    e.pos[1],
                    half.sin(),
                    half.cos(),
                    e.tile,
                    e.name
                );
            }
            s.push('\n');
        }
        for t in &self.tiles {
            let _ = writeln!(s, "[map]\n{}\n{}\n{}\n", t.x, t.y, t.file_name());
        }
        s
    }

    pub fn tile_map(tile: &Tile) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "File created with ets2-convert\n");
        let _ = writeln!(s, "[version]\n14\n");
        if tile.terrain.is_some() {
            let _ = writeln!(s, "[terrain]\n");
        }
        for sp in &tile.splines {
            // detail level, file, id, neighbours, x, height, y, heading, length, radius,
            // gradients, cant, skew, texture offset
            let _ = writeln!(
                s,
                "[spline]\n0\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n{}\n0\n0\n0\n0\n{}\n",
                sp.file,
                sp.id,
                sp.prev_id,
                sp.next_id,
                sp.pos[0],
                sp.pos[2],
                sp.pos[1],
                sp.heading,
                sp.length,
                sp.radius,
                sp.grad_start,
                sp.grad_end,
                sp.tex_offset
            );
        }
        s
    }

    pub fn ailists_cfg(&self) -> String {
        let mut s = String::new();
        for g in &self.ai_groups {
            let _ = writeln!(s, "[aigroup_2]\n{}\n", g.name);
            for (file, weight) in &g.vehicles {
                let _ = writeln!(s, "{file}\t{weight}");
            }
            let _ = writeln!(s, "[end]\n");
        }
        s
    }

    /// Writes the map into `dir` (created if missing). Files already there with the same
    /// names are replaced; others are left alone.
    pub fn write(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let put = |name: &str, bytes: Vec<u8>| -> Result<()> {
            let p = dir.join(name);
            fs::write(&p, bytes).with_context(|| format!("writing {}", p.display()))
        };
        put("global.cfg", utf16(&self.global_cfg()))?;
        for t in &self.tiles {
            put(&t.file_name(), utf16(&Self::tile_map(t)))?;
            if let Some(terrain) = &t.terrain {
                put(&format!("{}.terrain", t.file_name()), terrain.to_bytes())?;
            }
        }
        put("ailists.cfg", ansi(&self.ailists_cfg()))?;
        Ok(())
    }
}
