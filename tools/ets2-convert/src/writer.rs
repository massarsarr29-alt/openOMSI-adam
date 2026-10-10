//! Writes an OMSI 2 map folder: `global.cfg`, `tile_x_y.map` with its `.terrain`,
//! `ailists.cfg` and the timetable in `TTData`, in the layout `docs/FORMATS.md` describes
//! and `omsi-map` reads back.

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

/// A scenery object of a tile, in the tile's frame; its height is above the ground.
#[derive(Debug, Clone)]
pub struct Object {
    /// Relative to the OMSI root, e.g. `Sceneryobjects\Generic\bus_stop.sco`.
    pub file: String,
    pub id: i64,
    pub pos: [f64; 3],
    pub heading: f64,
    /// What the editor's Labels dialog shows: a bus stop's name and timetable data.
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Tile {
    pub x: i32,
    pub y: i32,
    pub splines: Vec<Spline>,
    pub objects: Vec<Object>,
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

impl EntryPoint {
    /// An entry point 2 m to the right of the start of a spline of tile `tile`, facing
    /// along it.
    pub fn beside(name: &str, tile: usize, s: &Spline) -> EntryPoint {
        let h = s.heading.to_radians();
        EntryPoint {
            name: name.into(),
            tile,
            pos: [
                s.pos[0] + 2.0 * h.cos(),
                s.pos[1] - 2.0 * h.sin(),
                s.pos[2] + 0.5,
            ],
            heading: s.heading,
        }
    }
}

/// A weighted list of AI vehicles (`[aigroup_2]`).
#[derive(Debug, Clone)]
pub struct AiGroup {
    pub name: String,
    pub vehicles: Vec<(String, u32)>,
}

/// A depot of timetable buses (`[aigroup_depot]`): the vehicle file and its fleet numbers
/// with their number plates.
#[derive(Debug, Clone)]
pub struct Depot {
    pub name: String,
    /// The `.hof` (destination list) the buses use, by its name.
    pub hof: String,
    pub vehicle: String,
    pub fleet: Vec<(String, String)>,
}

/// `TTData`: the stops, the ways between them and the lines that run on them.
#[derive(Debug, Clone, Default)]
pub struct Timetable {
    pub stops: Vec<Stop>,
    pub links: Vec<StnLink>,
    pub trips: Vec<Trip>,
    pub lines: Vec<Line>,
}

/// A stop of `Busstops.cfg`: its name and the bus stop object that stands for it.
#[derive(Debug, Clone)]
pub struct Stop {
    pub name: String,
    pub tile: usize,
    pub object_id: i64,
}

/// The way from one stop object to the next, as the lanes (spline, path, tile index, length)
/// a bus drives along.
#[derive(Debug, Clone)]
pub struct StnLink {
    pub from: i64,
    pub to: i64,
    pub steps: Vec<(i64, u32, usize, f64)>,
}

/// A trip (`.ttp`): the stop objects it calls at and how long it takes.
#[derive(Debug, Clone)]
pub struct Trip {
    pub name: String,
    pub terminus: String,
    pub line: String,
    pub stops: Vec<i64>,
    pub minutes: f64,
}

/// A tour of a line: its number, the depot its bus comes from, and the trips it drives with
/// their departures (minutes after midnight).
pub type Tour = (String, String, Vec<(String, f64)>);

/// A line (`.ttl`): its tours.
#[derive(Debug, Clone)]
pub struct Line {
    pub name: String,
    pub tours: Vec<Tour>,
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
    pub depots: Vec<Depot>,
    pub timetable: Timetable,
    /// Files written into the map folder as they are (a helper spline).
    pub extra_files: Vec<(String, String)>,
}

/// An invisible spline with one lane each way, 3.3 m to either side of the middle as on
/// Marcel's Seeburger street: for turns through junctions that are not drawn yet. Path 0
/// runs backwards, path 1 forwards.
pub const HELPER_SPLINE: &str =
    "[path]\n0\n-1.639\n0.100\n3.278\n1\n\n[path]\n0\n1.639\n0.100\n3.278\n0\n";

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

/// Other text files are Windows-1252 with CR LF (accented letters are where Latin-1 has
/// them).
fn ansi(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .chars()
        .map(|c| match c as u32 {
            u @ (0..=0x7F | 0xA0..=0xFF) => u as u8,
            _ => b'?',
        })
        .collect()
}

impl Map {
    fn next_id(&self) -> i64 {
        self.tiles
            .iter()
            .flat_map(|t| {
                t.splines
                    .iter()
                    .map(|s| s.id)
                    .chain(t.objects.iter().map(|o| o.id))
            })
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
        let _ = writeln!(s, "[ticketpack]\nTicketPacks\\Berlin_1\\Berlin_1.otp\n");
        if let Some(d) = self.depots.first() {
            let _ = writeln!(s, "[standarddepot]\n{}\n", d.name);
        }
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
        for o in &tile.objects {
            let _ = writeln!(
                s,
                "[object]\n0\n{}\n{}\n{}\n{}\n{}\n{}\n0\n0\n{}",
                o.file,
                o.id,
                o.pos[0],
                o.pos[1],
                o.pos[2],
                o.heading,
                o.labels.len()
            );
            for l in &o.labels {
                let _ = writeln!(s, "{l}");
            }
            s.push('\n');
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
        for d in &self.depots {
            let _ = writeln!(s, "[aigroup_depot]\n{}\n{}\n", d.name, d.hof);
            let _ = writeln!(s, "[aigroup_depot_typgroup_2]\n{}", d.vehicle);
            for (number, plate) in &d.fleet {
                let _ = writeln!(s, "{number}\t{plate}");
            }
            let _ = writeln!(s, "[end]\n");
        }
        s
    }

    pub fn busstops_cfg(&self) -> String {
        let mut s = String::from("File created with ets2-convert\n\n");
        for b in &self.timetable.stops {
            let _ = writeln!(
                s,
                "[busstop]\n{}\n{}\n{}\n0\n0\n0\n",
                b.name, b.tile, b.object_id
            );
        }
        s
    }

    pub fn stnlinks_cfg(&self) -> String {
        let mut s = String::from("File created with ets2-convert\n\n");
        for l in &self.timetable.links {
            let length: f64 = l.steps.iter().map(|e| e.3).sum();
            // (the six numbers after the stops are not used by Open OMSI; these are a stock
            // link's)
            let _ = writeln!(
                s,
                "[StnLink]\n{length:.3}\n{}\n{}\n2.42\n1.65\n10.3\n6.74\n0\n1\n",
                l.from, l.to
            );
            for (i, (id, path, tile, len)) in l.steps.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "   {i}:\n[StnLink_entry]\n{id}\n{path}\n{tile}\n{len:.3}\n-1\n0\n0\n"
                );
            }
        }
        s
    }

    pub fn trip_ttp(t: &Trip) -> String {
        let mut s = String::from("File created with ets2-convert\n\n");
        let _ = writeln!(s, "[trip]\n\n{}\n{}\n", t.terminus, t.line);
        for id in &t.stops {
            let _ = writeln!(s, "[station_typ2]\n{id}\n");
        }
        let _ = writeln!(s, "[profile]\nstandard\n{:.3}\n", t.minutes);
        s
    }

    pub fn line_ttl(l: &Line) -> String {
        let mut s =
            String::from("File created with ets2-convert\n\n[userallowed]\n\n[priority]\n1\n\n");
        for (number, group, trips) in &l.tours {
            let _ = writeln!(s, "[newtour]\n{number}\n{group}\n0\n");
            for (trip, dep) in trips {
                let _ = writeln!(s, "[addtrip]\n{trip}\n0\n{dep:.3}\n");
            }
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
        for (name, text) in &self.extra_files {
            put(name, ansi(text))?;
        }
        let tt = &self.timetable;
        if !tt.stops.is_empty() {
            let dir = dir.join("TTData");
            fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
            let put = |name: &str, text: String| -> Result<()> {
                let p = dir.join(name);
                fs::write(&p, ansi(&text)).with_context(|| format!("writing {}", p.display()))
            };
            put("Busstops.cfg", self.busstops_cfg())?;
            put("StnLinks.cfg", self.stnlinks_cfg())?;
            for t in &tt.trips {
                put(&format!("{}.ttp", t.name), Self::trip_ttp(t))?;
            }
            for l in &tt.lines {
                put(&format!("{}.ttl", l.name), Self::line_ttl(l))?;
            }
        }
        Ok(())
    }
}
