//! A sector's `.base` file: its roads, prefabs and the nodes they stand on. The other item
//! types of `.base` are read past (their layout still has to be right, as the file has no
//! item sizes); `parse` checks that the node list follows the last item.

use super::reader::Reader;
use super::token::Token;
use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;

/// Map format versions this reader follows: 901 (ETS 2 1.51-1.54) to 907 (1.59-1.61).
pub const VERSIONS: std::ops::RangeInclusive<u32> = 901..=907;

#[derive(Debug, Clone)]
pub struct Node {
    pub uid: u64,
    /// Metres, global: x east, y up, z south.
    pub pos: [f64; 3],
    /// Quaternion as stored: w, x, y, z.
    pub rot: [f32; 4],
    pub backward_item: u64,
    pub forward_item: u64,
    pub flags: u32,
}

#[derive(Debug, Clone)]
pub struct Road {
    pub uid: u64,
    pub flags: u32,
    pub road_look: Token,
    pub right_lanes_variant: Token,
    pub left_lanes_variant: Token,
    pub right_tmpl_look: Token,
    pub left_tmpl_look: Token,
    pub right_road_height: f32,
    pub left_road_height: f32,
    pub start_node: u64,
    pub end_node: u64,
    pub length: f32,
}

impl Road {
    pub fn no_ai(&self) -> bool {
        self.flags & (1 << 22) != 0
    }
}

#[derive(Debug, Clone)]
pub struct Prefab {
    pub uid: u64,
    pub flags: u32,
    pub model: Token,
    pub variant: Token,
    pub nodes: Vec<u64>,
    /// Index of the origin node in the prefab's `.ppd` (not in `nodes`).
    pub origin: u16,
    pub semaphores: Token,
}

/// A rectangle marking part of the map as a city (centred on its node).
#[derive(Debug, Clone)]
pub struct City {
    pub name: Token,
    pub width: f32,
    pub height: f32,
    pub node: u64,
}

#[derive(Debug, Clone, Default)]
pub struct Sector {
    pub version: u32,
    pub roads: Vec<Road>,
    pub prefabs: Vec<Prefab>,
    /// Bus stop prefab slaves: (item uid, node uid).
    pub bus_stops: Vec<(u64, u64)>,
    pub cities: Vec<City>,
    pub nodes: BTreeMap<u64, Node>,
    /// How many items of each type the file has.
    pub item_counts: BTreeMap<u32, usize>,
}

struct KdopItem {
    uid: u64,
    flags: u32,
}

fn kdop_item(r: &mut Reader) -> Result<KdopItem> {
    let uid = r.u64()?;
    r.skip(10 * 4)?; // bounding box: five minimums, five maximums
    let flags = r.u32()?;
    r.skip(1)?; // view distance
    Ok(KdopItem { uid, flags })
}

/// [[Vegetation struct]]: type, density, high-poly distance, scale, start, end.
fn vegetation(r: &mut Reader) -> Result<()> {
    r.skip(8 + 2 + 1 + 1 + 2 + 2)
}

/// [[Vegetation sphere struct]]s: centre, radius, type.
fn veg_spheres(r: &mut Reader) -> Result<()> {
    let n = r.count(20)?;
    r.skip(n * 20)
}

/// [[Quad info struct]]: materials, colours, quads, vertex offsets and normals.
fn quad_info(r: &mut Reader) -> Result<()> {
    let materials = r.u16()? as usize;
    r.skip(materials * (8 + 2))?;
    let colors = r.u16()? as usize;
    r.skip(colors * 4)?;
    r.skip(2 + 2)?; // grid size
    let quads = r.count(4)?;
    r.skip(quads * 4)?;
    for _ in 0..2 {
        let n = r.count(16)?;
        r.skip(n * 16)?;
    }
    Ok(())
}

/// [[Action Struct]] of triggers and cutscenes. A cutscene's actions have no name of their
/// own (seen in format 901: the parameters start right after the action count).
fn action(r: &mut Reader, named: bool) -> Result<()> {
    if named {
        r.token()?;
    }
    let floats = r.u32()?;
    // (u32::MAX: no custom parameters, and no flags after them either)
    if floats == u32::MAX {
        return Ok(());
    }
    {
        if floats as usize * 4 > r.remaining() {
            bail!("action with {floats} parameters");
        }
        r.skip(floats as usize * 4)?;
        let strings = r.count(8)?;
        for _ in 0..strings {
            r.string()?;
        }
        r.tokens()?;
        r.f32()?; // target range
    }
    r.u32()?; // action flags
    Ok(())
}

fn sign(r: &mut Reader, version: u32) -> Result<()> {
    r.token()?; // model
    r.u64()?; // node
    r.token()?; // look
    r.token()?; // variant
    let boards = r.u8()?;
    r.skip(boards as usize * 3 * 8)?;
    let template = r.string()?;
    if template.is_empty() {
        return Ok(());
    }
    if version >= 906 {
        let n = r.count(10)?;
        for _ in 0..n {
            r.token()?;
            let flags = r.u8()?;
            if flags & 1 != 0 {
                r.skip(2)?;
            }
            if flags & 2 != 0 {
                r.token()?;
            }
        }
    }
    let n = r.count(16)?;
    for _ in 0..n {
        r.u32()?; // id
        r.token()?; // area
        let attrs = r.count(6)?;
        for _ in 0..attrs {
            let ty = r.u16()?;
            r.u32()?; // index
            match ty {
                1 => r.skip(1)?,
                2..=4 => r.skip(4)?,
                5 => {
                    r.string()?;
                }
                6 => r.skip(8)?,
                _ => bail!("sign attribute of unknown type {ty}"),
            }
        }
    }
    Ok(())
}

/// Reads past the item of `ty` (its `kdop_item` already read), keeping roads, prefabs and
/// bus stops.
fn item(r: &mut Reader, s: &mut Sector, ty: u32, k: KdopItem) -> Result<()> {
    let v = s.version;
    match ty {
        // Road
        3 => {
            r.u32()?; // road flags
            let road_look = r.token()?;
            let right_lanes_variant = r.token()?;
            let left_lanes_variant = r.token()?;
            r.skip(2 * 8)?; // template variants
            r.skip(4 * 8)?; // edge models
            r.skip(2 * (8 + 4))?; // terrain profiles
            let right_tmpl_look = r.token()?;
            let left_tmpl_look = r.token()?;
            r.token()?; // material
            r.skip(3 * 2 * (8 + 2))?; // railings, three per side
            let right_road_height = r.i32()? as f32 / 100.0;
            let left_road_height = r.i32()? as f32 / 100.0;
            let start_node = r.u64()?;
            let end_node = r.u64()?;
            let length = r.f32()?;
            s.roads.push(Road {
                uid: k.uid,
                flags: k.flags,
                road_look,
                right_lanes_variant,
                left_lanes_variant,
                right_tmpl_look,
                left_tmpl_look,
                right_road_height,
                left_road_height,
                start_node,
                end_node,
                length,
            });
        }
        // Prefab
        4 => {
            let model = r.token()?;
            let variant = r.token()?;
            r.tokens()?; // additional parts
            let nodes = r.u64s()?;
            r.u64s()?; // slave items
            r.u64()?; // ferry link
            let origin = r.u16()?;
            r.skip(nodes.len() * (8 + 4))?; // terrain profile per node
            let semaphores = r.token()?;
            s.prefabs.push(Prefab {
                uid: k.uid,
                flags: k.flags,
                model,
                variant,
                nodes,
                origin,
                semaphores,
            });
        }
        // Company: economy link (city, prefab) from 903 on, name, node, spawn points
        6 => {
            if v >= 903 {
                r.token()?;
                r.u64()?;
            }
            r.token()?;
            if v < 903 {
                r.token()?;
                r.u64()?;
            }
            r.u64()?;
            // spawn point nodes, then one flag word per node (no second count)
            let n = r.count(12)?;
            r.skip(n * 8 + n * 4)?;
        }
        // Service, Fuel Pump: node, prefab, nodes
        7 | 35 => {
            r.u64()?;
            r.u64()?;
            r.u64s()?;
        }
        // Cut Plane
        8 => {
            r.u64s()?;
        }
        // City: name, width, height, node
        12 => s.cities.push(City {
            name: r.token()?,
            width: r.f32()?,
            height: r.f32()?,
            node: r.u64()?,
        }),
        // Map Overlay: look, node
        18 => r.skip(8 + 8)?,
        // Ferry: name, prefab, node, unload offset
        19 => r.skip(8 + 8 + 8 + 12)?,
        // Garage: city, buy mode, node, prefab, nodes
        22 => {
            r.skip(8 + 4 + 8 + 8)?;
            r.u64s()?;
        }
        // Trigger
        34 => {
            r.tokens()?;
            let nodes = r.u64s()?;
            let actions = r.count(12)?;
            for _ in 0..actions {
                action(r, true)?;
            }
            if nodes.len() == 1 {
                r.f32()?;
            }
        }
        // Sign (with a traffic rule)
        36 => sign(r, v)?,
        // Bus Stop: city, prefab (the economy link from 903 on), then the node
        37 => {
            r.token()?;
            r.u64()?;
            let node = r.u64()?;
            s.bus_stops.push((k.uid, node));
        }
        // Traffic Area: tags, nodes, rule, range
        38 => {
            r.tokens()?;
            r.u64s()?;
            r.token()?;
            r.f32()?;
        }
        // Trajectory
        41 => {
            r.u64s()?;
            r.token()?; // access rule
            let rules = r.count(16)?;
            for _ in 0..rules {
                r.u32()?;
                r.token()?;
                let n = r.count(4)?;
                r.skip(n * 4)?;
            }
            let checkpoints = r.count(16)?;
            r.skip(checkpoints * 16)?;
            r.tokens()?;
        }
        // Map Area: nodes, colour
        42 => {
            r.u64s()?;
            r.u32()?;
        }
        // Cutscene
        46 => {
            r.tokens()?;
            r.u64()?;
            let actions = r.count(4)?;
            for _ in 0..actions {
                action(r, false)?;
            }
        }
        // Visibility Area: node, half sizes, children
        48 => {
            r.skip(8 + 4 + 4)?;
            r.u64s()?;
        }
        // The items below belong in .aux, but are written to .base when a Far Model
        // parents them.
        // Terrain
        1 => {
            r.skip(8 + 8 + 12 + 12 + 4 + 4 + 4)?; // nodes, offsets, length, previous length, seed
            r.skip(3 * (8 + 2))?; // railings
            for _ in 0..2 {
                r.skip(2 + 8 + 4 + 8 + 4)?; // size, profile, previous profile
                for _ in 0..3 {
                    vegetation(r)?;
                }
                r.skip(2 + 2)?;
            }
            veg_spheres(r)?;
            quad_info(r)?;
            quad_info(r)?;
            r.skip(4 * 8)?; // edges and their looks
        }
        // Buildings: scheme, look, nodes, length, seed, stretch, height offsets
        2 => {
            r.skip(8 + 8)?;
            r.skip(2 * 8)?; // backward and forward node, no count
            r.skip(4 + 4 + 4)?;
            let n = r.count(4)?;
            r.skip(n * 4)?;
        }
        // Model: model, look, variant, parts, node, scale, terrain material, colour, rotation
        5 => {
            r.skip(3 * 8)?;
            r.tokens()?;
            r.skip(8 + 12 + 8 + 4 + 4)?;
        }
        // Bezier Patch: control points, tessellation, node, seed, vegetation, quads
        39 => {
            r.skip(16 * 12 + 2 + 2 + 8 + 4)?;
            // three vegetation structs of type, density and scale only (format 901)
            r.skip(3 * (8 + 2 + 1))?;
            veg_spheres(r)?;
            quad_info(r)?;
        }
        // Curve: up to 906 one curve model per item, from 907 up to four subcurves
        44 => {
            if v < 907 {
                r.token()?;
                r.skip(4 * 8)?; // nodes: backward, forward, two helpers; no count
                r.skip(4 + 4 + 4 + 4 + 4)?; // length, seed, stretch, scale, fixed step
                r.skip(8 + 4 + 4)?; // terrain material, colour, rotation
                r.skip(4 * 8)?; // first, last, centre part, look
                let n = r.count(4)?;
                r.skip(n * 4)?;
            } else {
                r.skip(4 * 8)?;
                r.f32()?;
                let mask = r.u32()?;
                for _ in 0..mask.count_ones() {
                    r.skip(8 + 4 + 4 + 4 + 4 + 4 + 8 + 4 + 4 + 4 * 8)?;
                    let n = r.count(4)?;
                    r.skip(n * 4)?;
                    r.skip(5 * 4)?;
                }
            }
        }
        _ => bail!("item type {ty} is not one of .base"),
    }
    Ok(())
}

impl Sector {
    pub fn parse_base(data: &[u8]) -> Result<Sector> {
        let mut r = Reader::new(data);
        let version = r.u32()?;
        if !VERSIONS.contains(&version) {
            bail!(
                "map format version {version} (this reader knows {}-{})",
                VERSIONS.start(),
                VERSIONS.end()
            );
        }
        r.token()?; // game id, "euro2"
        r.u32()?; // game map version
        let mut s = Sector {
            version,
            ..Default::default()
        };
        let items = r.count(57)?;
        // (type, start) of the items read so far: when one goes wrong, the error usually
        // shows only at the next item
        let mut read: Vec<(u32, usize)> = Vec::new();
        for i in 0..items {
            let at = r.pos;
            let ty = r.u32()?;
            let k = kdop_item(&mut r)?;
            item(&mut r, &mut s, ty, k).with_context(|| {
                let before: Vec<String> = read
                    .iter()
                    .rev()
                    .take(3)
                    .map(|(t, p)| format!("type {t} at {p}"))
                    .collect();
                format!(
                    "item {i} of {items} (type {ty}) at byte {at}, after {}",
                    before.join(", ")
                )
            })?;
            read.push((ty, at));
            *s.item_counts.entry(ty).or_default() += 1;
        }
        let nodes = r.count(8 + 12 + 16 + 16 + 4).context("node list")?;
        for _ in 0..nodes {
            let uid = r.u64()?;
            let pos = [
                r.i32()? as f64 / 256.0,
                r.i32()? as f64 / 256.0,
                r.i32()? as f64 / 256.0,
            ];
            let rot = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
            let backward_item = r.u64()?;
            let forward_item = r.u64()?;
            let flags = r.u32()?;
            s.nodes.insert(
                uid,
                Node {
                    uid,
                    pos,
                    rot,
                    backward_item,
                    forward_item,
                    flags,
                },
            );
        }
        let vis = r.count(8)?;
        r.skip(vis * 8)?;
        if r.remaining() != 0 {
            bail!("{} bytes left after the node list", r.remaining());
        }
        Ok(s)
    }
}
