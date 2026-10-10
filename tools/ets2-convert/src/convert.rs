//! Turns the roads of an ETS 2 map into OMSI 2 splines.
//!
//! An ETS 2 road runs between two nodes along a cubic Hermite curve whose tangents are the
//! nodes' directions scaled by the distance between them. OMSI splines are straights and
//! circular arcs, so each curve is sampled and the samples are joined by arcs, each arc
//! leaving in the direction the previous one arrived: the chain is smooth and passes
//! through every sample. Prefabs (junctions, roundabouts) are not drawn yet; their nodes
//! are joined by the same kind of curves so the road network stays connected: the main
//! ways through with the road spline, every other turn with an invisible helper spline that
//! is only put on the map where a bus line needs it (see `line`).

use crate::scs::sector::{City, Node, Prefab, Road, Sector};
use crate::writer::{AiGroup, EntryPoint, Map, Spline, Tile};
use omsi_map::terrain::Terrain;
use std::collections::{BTreeMap, HashMap, HashSet};

pub const TILE: f64 = 300.0;

/// Everything the converter needs from the sectors of a map.
#[derive(Debug, Default)]
pub struct World {
    pub nodes: HashMap<u64, Node>,
    pub roads: Vec<Road>,
    pub prefabs: Vec<Prefab>,
    pub cities: Vec<City>,
}

impl World {
    pub fn add(&mut self, s: Sector) {
        self.nodes.extend(s.nodes);
        self.roads.extend(s.roads);
        self.prefabs.extend(s.prefabs);
        self.cities.extend(s.cities);
    }

    /// The middle of all city areas with this name, in ETS 2 coordinates (x, z).
    pub fn city_centre(&self, name: &str) -> Option<[f64; 2]> {
        let ps: Vec<_> = self
            .cities
            .iter()
            .filter(|c| c.name.to_string() == name)
            .filter_map(|c| self.nodes.get(&c.node))
            .map(|n| [n.pos[0], n.pos[2]])
            .collect();
        if ps.is_empty() {
            return None;
        }
        let k = ps.len() as f64;
        Some([
            ps.iter().map(|p| p[0]).sum::<f64>() / k,
            ps.iter().map(|p| p[1]).sum::<f64>() / k,
        ])
    }
}

/// The part of the ETS 2 map to convert: a rectangle in ETS 2 x (east) and z (south).
#[derive(Debug, Clone, Copy)]
pub struct Area {
    pub min: [f64; 2],
    pub max: [f64; 2],
}

impl Area {
    /// A square of `tiles` × `tiles` OMSI tiles centred on an ETS 2 point.
    pub fn around(centre: [f64; 2], tiles: u32) -> Area {
        let half = tiles as f64 * TILE / 2.0;
        Area {
            min: [centre[0] - half, centre[1] - half],
            max: [centre[0] + half, centre[1] + half],
        }
    }

    pub fn contains(&self, p: &[f64; 3]) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[2])
    }

    /// From ETS 2 (x east, y up, z south) to the OMSI map frame (x east, y north, z up),
    /// with the south-west corner of the area at 0, 0.
    pub fn to_omsi(self, p: [f64; 3]) -> [f64; 3] {
        [p[0] - self.min[0], self.max[1] - p[2], p[1]]
    }

    pub fn tiles(&self) -> (i32, i32) {
        (
            ((self.max[0] - self.min[0]) / TILE).ceil() as i32,
            ((self.max[1] - self.min[1]) / TILE).ceil() as i32,
        )
    }
}

/// A node's direction in the OMSI frame. Node rotations are quaternions (w, x, y, z) that
/// turn ETS 2's forward axis, -z, into the direction the node points.
fn direction(n: &Node) -> [f64; 3] {
    let [w, x, y, z] = n.rot.map(f64::from);
    // the rotated -z axis
    let f = [
        -2.0 * (x * z + w * y),
        -2.0 * (y * z - w * x),
        -(1.0 - 2.0 * (x * x + y * y)),
    ];
    [f[0], -f[2], f[1]]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn neg(a: [f64; 3]) -> [f64; 3] {
    [-a[0], -a[1], -a[2]]
}

fn len2(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1])
}

/// OMSI heading of a direction: degrees, 0 = north, clockwise.
pub fn heading(d: [f64; 3]) -> f64 {
    d[0].atan2(d[1]).to_degrees()
}

/// Wraps an angle in degrees into -180..180.
pub fn wrap(a: f64) -> f64 {
    (a + 180.0).rem_euclid(360.0) - 180.0
}

/// A Hermite curve from `p0` leaving along `d0` to `p1` arriving along `d1`.
#[derive(Debug, Clone, Copy)]
pub struct Curve {
    pub p0: [f64; 3],
    pub d0: [f64; 3],
    pub p1: [f64; 3],
    pub d1: [f64; 3],
}

impl Curve {
    fn point(&self, t: f64) -> [f64; 3] {
        let l = len2(sub(self.p1, self.p0));
        let (t2, t3) = (t * t, t * t * t);
        let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
        let h10 = t3 - 2.0 * t2 + t;
        let h01 = -2.0 * t3 + 3.0 * t2;
        let h11 = t3 - t2;
        let at = |i: usize| {
            h00 * self.p0[i] + h10 * l * self.d0[i] + h01 * self.p1[i] + h11 * l * self.d1[i]
        };
        // height goes smoothly from end to end, ignoring the nodes' pitch
        [
            at(0),
            at(1),
            self.p0[2] + (self.p1[2] - self.p0[2]) * (3.0 * t2 - 2.0 * t3),
        ]
    }

    fn is_straight(&self) -> bool {
        let h = heading(sub(self.p1, self.p0));
        wrap(heading(self.d0) - h).abs() < 0.2 && wrap(heading(self.d1) - h).abs() < 0.2
    }

    /// The curve as a smooth chain of straights and arcs, in map coordinates, with ids,
    /// links and texture offsets left for the caller.
    pub fn splines(&self, file: &str) -> Vec<Spline> {
        let chord = len2(sub(self.p1, self.p0));
        let n = if self.is_straight() {
            1
        } else {
            (chord / 8.0).ceil().max(2.0) as usize
        };
        let pts: Vec<_> = (0..=n).map(|i| self.point(i as f64 / n as f64)).collect();
        let mut h = if self.is_straight() {
            heading(sub(self.p1, self.p0))
        } else {
            heading(self.d0)
        };
        let mut out = Vec::new();
        for w in pts.windows(2) {
            let c = sub(w[1], w[0]);
            let dist = len2(c);
            if dist < 0.01 {
                continue;
            }
            // an arc leaving along h through the next sample turns by twice the angle
            // between h and the chord
            let delta = wrap(heading(c) - h);
            if delta.abs() > 45.0 {
                // the curve doubles back on itself between samples: not a road to draw
                return Vec::new();
            }
            // below this the arc strays from the chord by about a centimetre
            let (length, radius) = if delta.abs() < 0.05 {
                (dist, 0.0)
            } else {
                let r = dist / (2.0 * delta.to_radians().abs().sin());
                (r * 2.0 * delta.to_radians().abs(), r * delta.signum())
            };
            let grade = (w[1][2] - w[0][2]) / length * 100.0;
            out.push(Spline {
                file: file.into(),
                id: 0,
                prev_id: 0,
                next_id: 0,
                pos: w[0],
                heading: h.rem_euclid(360.0),
                length,
                radius,
                grad_start: grade,
                grad_end: grade,
                tex_offset: 0.0,
            });
            h += 2.0 * delta;
        }
        out
    }
}

/// What to build and with which content.
pub struct Options {
    pub name: String,
    pub area: Area,
    /// The spline every road is drawn with, relative to the OMSI root.
    pub road_spline: String,
    /// The invisible spline of the turns through junctions that are not drawn, relative to
    /// the OMSI root (see `writer::HELPER_SPLINE`).
    pub helper_spline: String,
}

/// A way to drive from one node to another: a converted road, or a way through a junction.
#[derive(Debug, Clone)]
pub struct Link {
    pub from: u64,
    pub to: u64,
    /// Its splines from `from` to `to` in map coordinates (not the tile's), each with the
    /// index of its tile in `Map::tiles`.
    pub pieces: Vec<(usize, Spline)>,
    /// Drawn with the road spline; otherwise a helper spline, on the map only once `place`
    /// puts it there.
    pub visible: bool,
}

impl Link {
    pub fn length(&self) -> f64 {
        self.pieces.iter().map(|(_, s)| s.length).sum()
    }

    /// The path of its splines a vehicle drives along, forwards (from `from` to `to`) or
    /// backwards: the right-hand lane either way.
    pub fn path(&self, forward: bool) -> u32 {
        match (self.visible, forward) {
            // Seeburger street: 0 and 3 are the pavements
            (true, true) => 2,
            (true, false) => 1,
            (false, true) => 1,
            (false, false) => 0,
        }
    }
}

/// The ways the converted roads give, for routing bus lines on.
#[derive(Debug, Clone, Default)]
pub struct Network {
    pub links: Vec<Link>,
    /// The made-up nodes halfway round the U-turns through junctions (see `junction`).
    pub turnarounds: HashSet<u64>,
}

impl Network {
    /// Puts the splines of a helper link on the map (once).
    pub fn place(&self, map: &mut Map, link: usize) {
        let l = &self.links[link];
        if l.visible {
            return;
        }
        for (t, s) in &l.pieces {
            let tile = &mut map.tiles[*t];
            if tile.splines.iter().any(|x| x.id == s.id) {
                continue;
            }
            let mut s = s.clone();
            s.pos[0] -= tile.x as f64 * TILE;
            s.pos[1] -= tile.y as f64 * TILE;
            tile.splines.push(s);
        }
    }
}

/// A curve to convert: from node, to node, the curve, and whether it is drawn.
type Way = (u64, u64, Curve, bool);

/// One end of a converted road or connector: which spline, and whether it is its start.
#[derive(Clone, Copy)]
struct End {
    spline: usize,
    start: bool,
}

pub fn convert(world: &World, opt: &Options) -> (Map, Network) {
    let area = opt.area;
    let mut curves: Vec<Way> = Vec::new();
    let pos = |n: &Node| area.to_omsi(n.pos);
    for r in &world.roads {
        let (Some(a), Some(b)) = (world.nodes.get(&r.start_node), world.nodes.get(&r.end_node))
        else {
            continue;
        };
        if !area.contains(&a.pos) || !area.contains(&b.pos) {
            continue;
        }
        let chord = sub(pos(b), pos(a));
        // a few nodes point backwards along their road
        let orient = |d: [f64; 3]| {
            if d[0] * chord[0] + d[1] * chord[1] < 0.0 {
                neg(d)
            } else {
                d
            }
        };
        curves.push((
            r.start_node,
            r.end_node,
            Curve {
                p0: pos(a),
                d0: orient(direction(a)),
                p1: pos(b),
                d1: orient(direction(b)),
            },
            true,
        ));
    }
    let mut turnarounds = HashSet::new();
    for p in &world.prefabs {
        let (c, turns) = junction(world, &area, p);
        curves.extend(c);
        turnarounds.extend(turns);
    }

    // every curve becomes a chain of splines with ids from 1
    let mut all: Vec<Spline> = Vec::new();
    let mut ends: BTreeMap<u64, Vec<End>> = BTreeMap::new();
    let mut chains: Vec<(u64, u64, std::ops::Range<usize>, bool)> = Vec::new();
    for (from, to, c, visible) in &curves {
        let file = if *visible {
            &opt.road_spline
        } else {
            &opt.helper_spline
        };
        let chain = c.splines(file);
        if chain.is_empty() {
            continue;
        }
        let first = all.len();
        let mut offset = 0.0;
        for (i, mut s) in chain.into_iter().enumerate() {
            s.id = (first + i + 1) as i64;
            if i > 0 {
                s.prev_id = s.id - 1;
                all[first + i - 1].next_id = s.id;
            }
            s.tex_offset = offset;
            offset += s.length;
            all.push(s);
        }
        chains.push((*from, *to, first..all.len(), *visible));
        if !visible {
            continue;
        }
        ends.entry(*from).or_default().push(End {
            spline: first,
            start: true,
        });
        ends.entry(*to).or_default().push(End {
            spline: all.len() - 1,
            start: false,
        });
    }
    // chains meeting at a node are linked; where more than two meet, the first two are
    for e in ends.values().filter(|e| e.len() >= 2) {
        let (a, b) = (e[0], e[1]);
        let (ida, idb) = (all[a.spline].id, all[b.spline].id);
        let link = |s: &mut Spline, start: bool, id: i64| {
            if start {
                s.prev_id = id
            } else {
                s.next_id = id
            }
        };
        link(&mut all[a.spline], a.start, idb);
        link(&mut all[b.spline], b.start, ida);
    }

    let (nx, ny) = area.tiles();
    let mut tiles: Vec<Tile> = (0..ny)
        .flat_map(|y| {
            (0..nx).map(move |x| Tile {
                x,
                y,
                ..Default::default()
            })
        })
        .collect();
    // (a curve bulging out of the area stays on the edge tile, beyond its edge)
    let tile_of = |p: [f64; 3]| -> usize {
        let tx = ((p[0] / TILE).floor() as i32).clamp(0, nx - 1);
        let ty = ((p[1] / TILE).floor() as i32).clamp(0, ny - 1);
        (ty * nx + tx) as usize
    };
    let network = Network {
        links: chains
            .into_iter()
            .map(|(from, to, range, visible)| Link {
                from,
                to,
                pieces: all[range]
                    .iter()
                    .map(|s| (tile_of(s.pos), s.clone()))
                    .collect(),
                visible,
            })
            .collect(),
        turnarounds,
    };
    let samples: Vec<[f64; 3]> = network
        .links
        .iter()
        .filter(|l| l.visible)
        .flat_map(|l| l.pieces.iter().map(|(_, s)| s.pos))
        .collect();
    for l in network.links.iter().filter(|l| l.visible) {
        for (t, s) in &l.pieces {
            let mut s = s.clone();
            s.pos[0] -= tiles[*t].x as f64 * TILE;
            s.pos[1] -= tiles[*t].y as f64 * TILE;
            tiles[*t].splines.push(s);
        }
    }
    for t in &mut tiles {
        t.terrain = Some(terrain(t, &samples));
    }

    // the bus starts on the road nearest the middle of the area, on its right-hand side
    let mid = [
        (area.max[0] - area.min[0]) / 2.0,
        (area.max[1] - area.min[1]) / 2.0,
    ];
    let mut entry_points = Vec::new();
    if let Some((ti, s)) = tiles
        .iter()
        .enumerate()
        .flat_map(|(i, t)| t.splines.iter().map(move |s| (i, s)))
        .min_by(|(i, a), (j, b)| {
            let d = |t: usize, s: &Spline| {
                let gx = s.pos[0] + tiles[t].x as f64 * TILE - mid[0];
                let gy = s.pos[1] + tiles[t].y as f64 * TILE - mid[1];
                gx.hypot(gy)
            };
            d(*i, a).total_cmp(&d(*j, b))
        })
    {
        entry_points.push(EntryPoint::beside("Start", ti, s));
    }

    let map = Map {
        name: opt.name.clone(),
        friendly_name: opt.name.clone(),
        description: "Roads converted from an ETS 2 map by ets2-convert.".into(),
        ground_texture: ("texture\\gras.bmp".into(), "texture\\gras_det.bmp".into()),
        tiles,
        entry_points,
        ai_groups: vec![AiGroup {
            name: "NormalCars".into(),
            vehicles: vec![
                ("vehicles\\VW_Golf_2\\AI_VW_Golf_2.bus".into(), 70),
                ("vehicles\\MB_W123_230E\\AI_mb_w123_230e.bus".into(), 50),
                ("vehicles\\Citr_BX\\BX.ovh".into(), 40),
            ],
        }],
        ..Default::default()
    };
    (map, network)
}

/// Curves across a prefab joining its nodes: first the pairs that run nearly straight
/// through, then every node left over to the node it turns least towards - these are drawn.
/// Every other turn that is not a U-turn gets a hidden curve. A junction of four ways or
/// more may be a roundabout, so it has hidden U-turns as well: from each node round the
/// middle and back, through a made-up node beyond the middle (returned too).
fn junction(world: &World, area: &Area, p: &Prefab) -> (Vec<Way>, Vec<u64>) {
    let nodes: Vec<(u64, [f64; 3], [f64; 3])> = p
        .nodes
        .iter()
        .filter_map(|u| world.nodes.get(u).map(|n| (*u, n)))
        .filter(|(_, n)| area.contains(&n.pos))
        .map(|(u, n)| {
            let d = direction(n);
            // the direction into the prefab
            let inward = if n.forward_item == p.uid { d } else { neg(d) };
            (u, area.to_omsi(n.pos), inward)
        })
        .collect();
    if nodes.len() < 2 || nodes.len() > 8 {
        return (Vec::new(), Vec::new());
    }
    let turn = |i: usize, j: usize| wrap(heading(nodes[i].2) - heading(neg(nodes[j].2))).abs();
    let mut pairs: Vec<(usize, usize)> = (0..nodes.len())
        .flat_map(|i| (i + 1..nodes.len()).map(move |j| (i, j)))
        .collect();
    pairs.sort_by(|a, b| turn(a.0, a.1).total_cmp(&turn(b.0, b.1)));
    let mut used = vec![false; nodes.len()];
    let mut joined = Vec::new();
    let mut out = Vec::new();
    let curve = |i: usize, j: usize| {
        let (a, b) = (&nodes[i], &nodes[j]);
        (len2(sub(b.1, a.1)) <= 120.0).then_some((
            a.0,
            b.0,
            Curve {
                p0: a.1,
                d0: a.2,
                p1: b.1,
                d1: neg(b.2),
            },
        ))
    };
    let mut join = |i: usize, j: usize, visible: bool| {
        joined.push((i.min(j), i.max(j)));
        if let Some((a, b, c)) = curve(i, j) {
            out.push((a, b, c, visible));
        }
    };
    for &(i, j) in &pairs {
        if !used[i] && !used[j] && turn(i, j) < 50.0 {
            used[i] = true;
            used[j] = true;
            join(i, j, true);
        }
    }
    for i in 0..nodes.len() {
        if !used[i] {
            let j = (0..nodes.len())
                .filter(|&j| j != i && len2(sub(nodes[j].1, nodes[i].1)) > 2.0)
                .min_by(|&a, &b| turn(i, a).total_cmp(&turn(i, b)));
            if let Some(j) = j {
                used[i] = true;
                join(i, j, true);
            }
        }
    }
    for &(i, j) in &pairs {
        if !joined.contains(&(i, j))
            && turn(i, j) < 135.0
            && len2(sub(nodes[j].1, nodes[i].1)) > 2.0
        {
            if let Some((a, b, c)) = curve(i, j) {
                out.push((a, b, c, false));
            }
        }
    }
    let mut turns = Vec::new();
    if nodes.len() >= 4 {
        let k = nodes.len() as f64;
        let mid = [
            nodes.iter().map(|n| n.1[0]).sum::<f64>() / k,
            nodes.iter().map(|n| n.1[1]).sum::<f64>() / k,
            nodes.iter().map(|n| n.1[2]).sum::<f64>() / k,
        ];
        let radius = nodes.iter().map(|n| len2(sub(n.1, mid))).sum::<f64>() / k;
        let radius = (radius * 0.5).max(8.0);
        for (i, (u, pos, d)) in nodes.iter().enumerate() {
            let n = len2(*d).max(1e-9);
            let d = [d[0] / n, d[1] / n, 0.0];
            // beyond the middle, going round it anticlockwise
            let far = [mid[0] + d[0] * radius, mid[1] + d[1] * radius, mid[2]];
            let left = [-d[1], d[0], 0.0];
            let id = p.uid.rotate_left(17) ^ (i as u64 + 1);
            out.push((
                *u,
                id,
                Curve {
                    p0: *pos,
                    d0: d,
                    p1: far,
                    d1: left,
                },
                false,
            ));
            out.push((
                id,
                *u,
                Curve {
                    p0: far,
                    d0: left,
                    p1: *pos,
                    d1: neg(d),
                },
                false,
            ));
            turns.push(id);
        }
    }
    (out, turns)
}

/// Ground a little below the nearest road, so roads are never buried.
fn terrain(tile: &Tile, samples: &[[f64; 3]]) -> Terrain {
    let mut t = Terrain::flat();
    let (ox, oy) = (tile.x as f64 * TILE, tile.y as f64 * TILE);
    let near: Vec<_> = samples
        .iter()
        .filter(|p| {
            (ox - 150.0..ox + TILE + 150.0).contains(&p[0])
                && (oy - 150.0..oy + TILE + 150.0).contains(&p[1])
        })
        .collect();
    let fallback = samples.iter().map(|p| p[2]).fold(f64::MAX, f64::min);
    let fallback = if fallback == f64::MAX { 0.0 } else { fallback };
    let n = t.samples();
    for iy in 0..n {
        for ix in 0..n {
            let x = ox + ix as f64 * TILE / t.cells as f64;
            let y = oy + iy as f64 * TILE / t.cells as f64;
            let d = |p: &[f64; 3]| (p[0] - x).hypot(p[1] - y);
            let close = near
                .iter()
                .filter(|p| d(p) < 15.0)
                .map(|p| p[2])
                .fold(f64::MAX, f64::min);
            let h = if close < f64::MAX {
                close
            } else {
                near.iter()
                    .min_by(|a, b| d(a).total_cmp(&d(b)))
                    .map(|p| p[2])
                    .unwrap_or(fallback)
            };
            t.heights[iy * n + ix] = (h - 0.3) as f32;
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end_of(chain: &[Spline]) -> ([f64; 2], f64) {
        chain.last().unwrap().end()
    }

    #[test]
    fn straight_road_is_one_spline() {
        let c = Curve {
            p0: [0.0, 0.0, 5.0],
            d0: [1.0, 0.0, 0.0],
            p1: [50.0, 0.0, 6.0],
            d1: [1.0, 0.0, 0.0],
        };
        let s = c.splines("road.sli");
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].length, s[0].radius, s[0].heading), (50.0, 0.0, 90.0));
        assert!((s[0].grad_start - 2.0).abs() < 1e-9);
    }

    #[test]
    fn bend_ends_on_its_end_node() {
        // a quarter turn to the right: leaving north, arriving heading east
        let c = Curve {
            p0: [0.0, 0.0, 0.0],
            d0: [0.0, 1.0, 0.0],
            p1: [40.0, 40.0, 0.0],
            d1: [1.0, 0.0, 0.0],
        };
        let s = c.splines("road.sli");
        assert!(s.len() > 2);
        assert!(s.iter().all(|s| s.radius > 0.0), "turns right throughout");
        let (end, h) = end_of(&s);
        assert!(
            (end[0] - 40.0).abs() < 1e-6 && (end[1] - 40.0).abs() < 1e-6,
            "{end:?}"
        );
        assert!(wrap(h - 90.0).abs() < 3.0, "arrives heading {h}");
        // each piece starts where the previous one ends
        for w in s.windows(2) {
            let (e, h) = w[0].end();
            assert!((e[0] - w[1].pos[0]).hypot(e[1] - w[1].pos[1]) < 1e-6);
            assert!(wrap(h - w[1].heading).abs() < 1e-6);
        }
    }

    #[test]
    fn node_rotation_gives_direction() {
        // a yaw of 90 degrees turns ETS 2's forward (-z, north) to the west
        let half = std::f32::consts::FRAC_PI_4;
        let n = Node {
            uid: 1,
            pos: [0.0; 3],
            rot: [half.cos(), 0.0, half.sin(), 0.0],
            backward_item: 0,
            forward_item: 0,
            flags: 0,
        };
        let d = direction(&n);
        assert!((d[0] + 1.0).abs() < 1e-6 && d[1].abs() < 1e-6, "{d:?}");
    }
}
