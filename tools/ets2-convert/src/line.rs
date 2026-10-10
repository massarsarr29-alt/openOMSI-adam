//! A bus line on the converted roads: its stops, put where the map's stop poles stand, the
//! way the bus drives between them, found on the road network, and its timetable.
//!
//! A stop is put on the road nearest its pole, in the direction that has the pole on its
//! right. From one stop to the next the bus takes the shortest way that never turns by more
//! than 60° where two ways meet (the turns proper are the junctions' own ways); the
//! junction turns it takes that are not drawn are put on the map as helper splines.

use crate::convert::{wrap, Area, Network, TILE};
use crate::writer::{Depot, EntryPoint, Line, Map, Object, Spline, StnLink, Stop, Trip};
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

/// A line to build: its number and its stops in the order of the outward trip, each with
/// its pole on the outward side and, when the map has one, on the way back (ETS 2 x, z).
pub struct LineSpec {
    pub number: String,
    pub stops: Vec<StopSpec>,
    /// Minutes from the first stop to the last.
    pub minutes: f64,
    /// The buses that run it: vehicle file relative to the OMSI root, and its `.hof`.
    pub vehicle: String,
    pub hof: String,
}

pub struct StopSpec {
    pub name: String,
    pub outward: [f64; 2],
    pub back: Option<[f64; 2]>,
    /// Only roughly where the stop is (no pole known): it goes on the nearest road, the
    /// way that leads towards the next stop, beside the road.
    pub rough: bool,
}

impl LineSpec {
    /// Reads a line file: the line number on the first line, then a stop per line as
    /// `name; x, z` or `name; x, z; x, z` (the pole on the way back), or `name; ~x, z` for
    /// a stop known only roughly. Empty lines and lines starting with `#` are skipped; the
    /// minutes are given by a `minutes: 35` line.
    pub fn parse(text: &str) -> anyhow::Result<LineSpec> {
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'));
        let number = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("the line file is empty"))?
            .to_string();
        let mut spec = LineSpec {
            number,
            stops: Vec::new(),
            minutes: 0.0,
            vehicle: "Vehicles\\MAN_NL_NG\\MAN_GN92_main.bus".into(),
            hof: "Grundorf".into(),
        };
        let point = |s: &str| -> anyhow::Result<[f64; 2]> {
            let v: Vec<f64> = s
                .split(',')
                .map(|v| v.trim().parse())
                .collect::<Result<_, _>>()?;
            anyhow::ensure!(v.len() == 2, "a pole is \"x, z\": {s}");
            Ok([v[0], v[1]])
        };
        for l in lines {
            if let Some(m) = l.strip_prefix("minutes:") {
                spec.minutes = m.trim().parse()?;
                continue;
            }
            let f: Vec<&str> = l.split(';').map(str::trim).collect();
            anyhow::ensure!(f.len() >= 2, "a stop is \"name; x, z\": {l}");
            let rough = f[1].starts_with('~');
            spec.stops.push(StopSpec {
                name: f[0].to_string(),
                rough,
                outward: point(f[1].trim_start_matches('~'))?,
                back: f.get(2).map(|p| point(p)).transpose()?,
            });
        }
        Ok(spec)
    }
}

/// A place on the network: a link, the way it is driven, and how far along it in that
/// direction.
#[derive(Debug, Clone, Copy)]
struct At {
    link: usize,
    forward: bool,
    along: f64,
}

/// One way a stop may be served: where on the network, where its object stands and which
/// way it faces, and the point of the road by it.
#[derive(Debug, Clone, Copy)]
struct Spot {
    at: At,
    pos: [f64; 2],
    heading: f64,
    road: [f64; 2],
}

/// A stop of one direction, served the first of its ways that the bus can reach.
struct Place {
    name: String,
    spots: Vec<Spot>,
}

/// Where a spline is `s` metres along it, and its heading there.
fn point_on(sp: &Spline, s: f64) -> ([f64; 2], f64) {
    Spline {
        length: s,
        ..sp.clone()
    }
    .end()
}

/// The drawn roads near `p` (map coordinates), nearest first, at most six and within
/// 30 m: where along which link, which way has `p` on its right, the point on the road, the
/// heading there that way, and the distance.
fn snaps(net: &Network, p: [f64; 2]) -> Vec<(At, [f64; 2], f64, f64)> {
    let mut found: Vec<(At, [f64; 2], f64, f64)> = Vec::new();
    for (li, l) in net.links.iter().enumerate().filter(|(_, l)| l.visible) {
        let total = l.length();
        let mut before = 0.0;
        let mut best: Option<(At, [f64; 2], f64, f64)> = None;
        for (_, sp) in &l.pieces {
            let reach = sp.length + 40.0;
            if (sp.pos[0] - p[0]).hypot(sp.pos[1] - p[1]) < reach {
                let n = sp.length.ceil().max(1.0) as usize;
                for k in 0..=n {
                    let s = sp.length * k as f64 / n as f64;
                    let (q, h) = point_on(sp, s);
                    let d = (q[0] - p[0]).hypot(q[1] - p[1]);
                    if d < 30.0 && best.as_ref().is_none_or(|b| d < b.3) {
                        let along = before + s;
                        // the pole is right of the spline's direction, or left of it
                        let r = h.to_radians();
                        let right = (p[0] - q[0]) * r.cos() - (p[1] - q[1]) * r.sin() > 0.0;
                        let at = At {
                            link: li,
                            forward: right,
                            along: if right { along } else { total - along },
                        };
                        best = Some((at, q, if right { h } else { h + 180.0 }, d));
                    }
                }
            }
            before += sp.length;
        }
        found.extend(best);
    }
    found.sort_by(|a, b| a.3.total_cmp(&b.3));
    found.truncate(6);
    found
}

/// Heading at the start and at the end of a link driven one way.
fn ends(net: &Network, link: usize, forward: bool) -> (f64, f64) {
    let l = &net.links[link];
    let first = l.pieces[0].1.heading;
    let last = l.pieces.last().unwrap().1.end().1;
    if forward {
        (first, last)
    } else {
        (last + 180.0, first + 180.0)
    }
}

#[derive(PartialEq)]
struct Item(f64, usize);
impl Eq for Item {}
impl PartialOrd for Item {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Item {
    fn cmp(&self, o: &Self) -> Ordering {
        o.0.total_cmp(&self.0)
    }
}

/// The ways driven from `a` to `b`: links and directions, the first `a`'s and the last `b`'s.
fn route(
    net: &Network,
    starts: &HashMap<u64, Vec<(usize, bool)>>,
    a: At,
    b: At,
) -> Option<Vec<(usize, bool)>> {
    if a.link == b.link && a.forward == b.forward && b.along >= a.along {
        return Some(vec![(a.link, a.forward)]);
    }
    let state = |link: usize, fwd: bool| link * 2 + usize::from(!fwd);
    let n = net.links.len() * 2;
    let mut dist = vec![f64::INFINITY; n];
    let mut prev = vec![usize::MAX; n];
    let s0 = state(a.link, a.forward);
    let goal = state(b.link, b.forward);
    dist[s0] = 0.0;
    let mut heap = BinaryHeap::from([Item(0.0, s0)]);
    while let Some(Item(d, s)) = heap.pop() {
        if d > dist[s] {
            continue;
        }
        if s == goal && s != s0 {
            break;
        }
        let (link, fwd) = (s / 2, s % 2 == 0);
        let l = &net.links[link];
        let node = if fwd { l.to } else { l.from };
        let h_end = ends(net, link, fwd).1;
        for &(next, nfwd) in starts.get(&node).into_iter().flatten() {
            if next == link {
                continue;
            }
            if wrap(ends(net, next, nfwd).0 - h_end).abs() > 60.0 {
                continue;
            }
            let t = state(next, nfwd);
            // a U-turn round a junction only when there is no other way
            let l = &net.links[next];
            let turn = [l.from, l.to].iter().any(|n| net.turnarounds.contains(n));
            let nd = d + l.length() + if turn { 250.0 } else { 0.0 };
            if nd < dist[t] {
                dist[t] = nd;
                prev[t] = s;
                heap.push(Item(nd, t));
            }
        }
    }
    if prev[goal] == usize::MAX {
        return None;
    }
    let mut out = vec![goal];
    while *out.last().unwrap() != s0 {
        out.push(prev[*out.last().unwrap()]);
    }
    out.reverse();
    Some(out.into_iter().map(|s| (s / 2, s % 2 == 0)).collect())
}

/// The lanes from `a` to `b` along `ways`: (spline id, path, tile index, length).
fn steps(net: &Network, ways: &[(usize, bool)], a: At, b: At) -> Vec<(i64, u32, usize, f64)> {
    let mut out = Vec::new();
    for (k, &(link, fwd)) in ways.iter().enumerate() {
        let l = &net.links[link];
        let mut pieces: Vec<&(usize, Spline)> = l.pieces.iter().collect();
        if !fwd {
            pieces.reverse();
        }
        let mut along = 0.0;
        for (t, sp) in pieces {
            let (s0, s1) = (along, along + sp.length);
            along = s1;
            // only from the piece of the first stop on, up to the piece of the next stop
            if k == 0 && s1 < a.along {
                continue;
            }
            if k == ways.len() - 1 && s0 > b.along {
                break;
            }
            out.push((sp.id, l.path(fwd), *t, sp.length));
        }
    }
    out
}

/// The index in `Map::tiles` of the tile under `p` (map coordinates), and `p` in its frame.
fn tile_at(map: &Map, p: [f64; 2]) -> (usize, [f64; 2]) {
    let (tx, ty) = ((p[0] / TILE).floor() as i32, (p[1] / TILE).floor() as i32);
    let i = map
        .tiles
        .iter()
        .position(|t| t.x == tx && t.y == ty)
        .unwrap_or_else(|| {
            // beyond the area: the nearest tile, with the point beyond its edge
            (0..map.tiles.len())
                .min_by_key(|&i| (map.tiles[i].x - tx).abs() + (map.tiles[i].y - ty).abs())
                .unwrap()
        });
    let t = &map.tiles[i];
    (i, [p[0] - t.x as f64 * TILE, p[1] - t.y as f64 * TILE])
}

/// What became of the line: a line of text per thing worth knowing.
pub type Report = Vec<String>;

/// Adds the line to the map: its stop objects, `TTData` and a depot of buses.
pub fn build(map: &mut Map, net: &Network, area: &Area, spec: &LineSpec) -> Report {
    let mut report = Report::new();
    let to_map = |p: [f64; 2]| {
        let q = area.to_omsi([p[0], 0.0, p[1]]);
        [q[0], q[1]]
    };
    if let Ok(path) = std::env::var("ETS2_DUMP_LINKS") {
        // every way as "index from to visible x y x y ...", for drawing
        let mut out = String::new();
        for (i, l) in net.links.iter().enumerate() {
            out += &format!("{i} {} {} {}", l.from, l.to, l.visible);
            for (_, sp) in &l.pieces {
                out += &format!(" {:.1} {:.1}", sp.pos[0], sp.pos[1]);
            }
            let e = l.pieces.last().unwrap().1.end().0;
            out += &format!(" {:.1} {:.1}\n", e[0], e[1]);
        }
        let _ = std::fs::write(path, out);
    }
    let mut starts: HashMap<u64, Vec<(usize, bool)>> = HashMap::new();
    for (i, l) in net.links.iter().enumerate() {
        starts.entry(l.from).or_default().push((i, true));
        starts.entry(l.to).or_default().push((i, false));
    }
    let mut next_id = map
        .tiles
        .iter()
        .flat_map(|t| t.splines.iter().map(|s| s.id))
        .chain(
            net.links
                .iter()
                .flat_map(|l| l.pieces.iter().map(|p| p.1.id)),
        )
        .max()
        .unwrap_or(0)
        + 1;

    // where each stop of each direction may be: on the road by its pole the way that has
    // the pole on its right (or, for a rough place, the way to the next stop), else the
    // other way, opposite the pole
    let flip = |at: At| At {
        forward: !at.forward,
        along: net.links[at.link].length() - at.along,
        ..at
    };
    let mut directions: Vec<Vec<Option<Place>>> = Vec::new();
    for back in [false, true] {
        let order: Vec<&StopSpec> = if back {
            spec.stops.iter().rev().collect()
        } else {
            spec.stops.iter().collect()
        };
        let mut places = Vec::new();
        for (k, st) in order.iter().enumerate() {
            let pole = to_map(if back {
                st.back.unwrap_or(st.outward)
            } else {
                st.outward
            });
            let found = snaps(net, pole);
            if found.is_empty() {
                if !back {
                    report.push(format!("{}: no road within 30 m of its pole", st.name));
                }
                places.push(None);
                continue;
            }
            if found[0].3 > 15.0 && !st.rough && !back {
                report.push(format!(
                    "{}: its pole stands {:.0} m from the road",
                    st.name, found[0].3
                ));
            }
            // (a pole by a lay-by that is not joined to the road has the road a little
            // further away as well)
            let mut spots = Vec::new();
            for (mut at, q, mut h, _) in found {
                let mut pos = pole;
                if st.rough {
                    // the way that heads for the next stop (away from the one before, at the end)
                    let (other, sign) = match order.get(k + 1) {
                        Some(n) => (to_map(n.outward), 1.0),
                        None => (to_map(order[k.saturating_sub(1)].outward), -1.0),
                    };
                    let r = h.to_radians();
                    let toward = (other[0] - q[0]) * r.sin() + (other[1] - q[1]) * r.cos();
                    if toward * sign < 0.0 {
                        at = flip(at);
                        h += 180.0;
                    }
                    // on the pavement, right of the way
                    let r = h.to_radians();
                    pos = [q[0] + 7.0 * r.cos(), q[1] - 7.0 * r.sin()];
                } else if back && st.back.is_none() {
                    // only a pole on the other side: the stop is the other way round, opposite it
                    at = flip(at);
                    h += 180.0;
                    pos = [2.0 * q[0] - pole[0], 2.0 * q[1] - pole[1]];
                }
                spots.push(Spot {
                    at,
                    pos,
                    heading: h,
                    road: q,
                });
                spots.push(Spot {
                    at: flip(at),
                    pos: [2.0 * q[0] - pos[0], 2.0 * q[1] - pos[1]],
                    heading: h + 180.0,
                    road: q,
                });
            }
            // the preferred way of each road before the other way of any
            let (first, second): (Vec<_>, Vec<_>) =
                spots.into_iter().enumerate().partition(|(i, _)| i % 2 == 0);
            places.push(Some(Place {
                name: st.name.clone(),
                spots: first.into_iter().chain(second).map(|(_, s)| s).collect(),
            }));
        }
        directions.push(places);
    }

    // the way of each direction: from each stop the first of its spots the bus can reach
    let mut trips = Vec::new();
    let mut entry: Option<EntryPoint> = None;
    for (back, places) in directions.iter().enumerate() {
        let back = back == 1;
        let places: Vec<&Place> = places.iter().flatten().collect();
        // the spots that give the shortest trip, counting each spot after a stop's first as
        // 60 m more (its pole on the wrong side, or a road further from it); where no way
        // leads on, the trip starts afresh
        type Leg = Option<(usize, Vec<(usize, bool)>)>;
        let penalty = |j: usize| j as f64 * 60.0;
        let mut cost: Vec<Vec<f64>> = vec![(0..places[0].spots.len()).map(penalty).collect()];
        let mut from: Vec<Vec<Leg>> = vec![vec![None; places[0].spots.len()]];
        for k in 1..places.len() {
            let (mut ck, mut fk) = (Vec::new(), Vec::new());
            for j in 0..places[k].spots.len() {
                let mut best: (f64, Leg) = (f64::INFINITY, None);
                for (i, &c) in cost[k - 1].iter().enumerate() {
                    if !c.is_finite() || c >= best.0 {
                        continue;
                    }
                    let (a, b) = (places[k - 1].spots[i].at, places[k].spots[j].at);
                    if let Some(w) = route(net, &starts, a, b) {
                        let len: f64 = w.iter().map(|&(l, _)| net.links[l].length()).sum();
                        let total = c + len + penalty(j);
                        if total < best.0 {
                            best = (total, Some((i, w)));
                        }
                    }
                }
                ck.push(best.0);
                fk.push(best.1);
            }
            if ck.iter().all(|c| !c.is_finite()) {
                report.push(format!(
                    "{} -> {}: no way found{}",
                    places[k - 1].name,
                    places[k].name,
                    if back { " (way back)" } else { "" }
                ));
                let low = cost[k - 1].iter().copied().fold(f64::INFINITY, f64::min);
                ck = (0..places[k].spots.len())
                    .map(|j| low + penalty(j))
                    .collect();
            }
            cost.push(ck);
            from.push(fk);
        }
        let argmin = |c: &[f64]| {
            (0..c.len())
                .min_by(|&a, &b| c[a].total_cmp(&c[b]))
                .unwrap_or(0)
        };
        let mut chosen = vec![0; places.len()];
        let mut legs: Vec<Option<Vec<(usize, bool)>>> = vec![None; places.len().saturating_sub(1)];
        if let Some(last) = cost.last() {
            let mut j = argmin(last);
            for k in (1..places.len()).rev() {
                chosen[k] = j;
                match &from[k][j] {
                    Some((i, w)) => {
                        legs[k - 1] = Some(w.clone());
                        j = *i;
                    }
                    None => j = argmin(&cost[k - 1]),
                }
            }
            chosen[0] = j;
        }
        let mut ids = Vec::new();
        for (k, p) in places.iter().enumerate() {
            let s = &p.spots[chosen[k]];
            let (tile, local) = tile_at(map, s.pos);
            let id = next_id;
            next_id += 1;
            map.tiles[tile].objects.push(Object {
                file: "Sceneryobjects\\Generic\\bus_stop.sco".into(),
                id,
                pos: [local[0], local[1], 0.0],
                heading: s.heading.rem_euclid(360.0),
                labels: vec![
                    p.name.clone(),
                    "0".into(),
                    "0".into(),
                    "10".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ],
            });
            map.timetable.stops.push(Stop {
                name: p.name.clone(),
                tile,
                object_id: id,
            });
            if !back && k == 0 {
                // the player's bus starts at the first stop, in its lane, a little before it
                let r = s.heading.to_radians();
                let q = [
                    s.road[0] + 1.639 * r.cos() - 15.0 * r.sin(),
                    s.road[1] - 1.639 * r.sin() - 15.0 * r.cos(),
                ];
                let (t, local) = tile_at(map, q);
                let z = net.links[s.at.link].pieces[0].1.pos[2];
                entry = Some(EntryPoint {
                    name: p.name.clone(),
                    tile: t,
                    pos: [local[0], local[1], z + 0.5],
                    heading: s.heading.rem_euclid(360.0),
                });
            }
            ids.push(id);
        }
        for (k, leg) in legs.iter().enumerate() {
            let Some(ways) = leg else { continue };
            for &(link, _) in ways {
                net.place(map, link);
            }
            let (a, b) = (
                places[k].spots[chosen[k]].at,
                places[k + 1].spots[chosen[k + 1]].at,
            );
            map.timetable.links.push(StnLink {
                from: ids[k],
                to: ids[k + 1],
                steps: steps(net, ways, a, b),
            });
        }
        let n: Vec<&str> = places.iter().map(|p| p.name.as_str()).collect();
        let short = |s: &str| {
            s.split(" - ")
                .last()
                .unwrap_or(s)
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
        };
        trips.push(Trip {
            name: format!("{}_{}-{}", spec.number, short(n[0]), short(n[n.len() - 1])),
            terminus: n[n.len() - 1].to_string(),
            line: spec.number.clone(),
            stops: ids.clone(),
            minutes: spec.minutes,
        });
    }

    if let Some(e) = entry {
        map.entry_points.insert(0, e);
    }

    // buses every quarter of an hour or so from 5:30 to 21:30, each driving out and back
    // with a five minute break at either end
    let cycle = 2.0 * (spec.minutes + 5.0);
    let buses = (cycle / 15.0).ceil().max(1.0) as usize;
    let depot = format!("Ligne {}", spec.number);
    let mut tours = Vec::new();
    for k in 0..buses {
        let mut list = Vec::new();
        let mut t = 330.0 + k as f64 * cycle / buses as f64;
        while t < 1290.0 {
            list.push((trips[0].name.clone(), t));
            list.push((trips[1].name.clone(), t + spec.minutes + 5.0));
            t += cycle;
        }
        tours.push((format!("{}", k + 1), depot.clone(), list));
    }
    map.depots.push(Depot {
        name: depot,
        hof: spec.hof.clone(),
        vehicle: spec.vehicle.clone(),
        fleet: (1..=buses)
            .map(|k| {
                (
                    format!("{}{k:02}", spec.number),
                    format!("{}-{k:02}", spec.number),
                )
            })
            .collect(),
    });
    map.timetable.lines.push(Line {
        name: spec.number.clone(),
        tours,
    });
    report.push(format!(
        "line {}: {} stops, {} of {} ways between them found, {buses} buses",
        spec.number,
        spec.stops.len(),
        map.timetable.links.len(),
        directions
            .iter()
            .map(|d| d.iter().flatten().count().saturating_sub(1))
            .sum::<usize>()
    ));
    map.timetable.trips.extend(trips);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::Link;
    use crate::writer::Tile;

    fn straight(id: i64, y: f64) -> (usize, Spline) {
        (
            0,
            Spline {
                file: "road.sli".into(),
                id,
                prev_id: 0,
                next_id: 0,
                pos: [100.0, y, 0.0],
                heading: 0.0,
                length: 100.0,
                radius: 0.0,
                grad_start: 0.0,
                grad_end: 0.0,
                tex_offset: 0.0,
            },
        )
    }

    #[test]
    fn line_file_is_read() {
        let s = LineSpec::parse("# a line\n114\nminutes: 35\nA; 1, 2\nB; 3, 4; 5, 6\nC; ~7, 8\n")
            .unwrap();
        assert_eq!(
            (s.number.as_str(), s.minutes, s.stops.len()),
            ("114", 35.0, 3)
        );
        assert_eq!(s.stops[1].back, Some([5.0, 6.0]));
        assert!(s.stops[2].rough && !s.stops[0].rough);
    }

    #[test]
    fn stops_on_a_straight_road_are_joined_both_ways() {
        // a road north from (100, 0) to (100, 200) in two pieces, a pole right of it at
        // each end
        let net = Network {
            links: vec![
                Link {
                    from: 1,
                    to: 2,
                    pieces: vec![straight(1, 0.0)],
                    visible: true,
                },
                Link {
                    from: 2,
                    to: 3,
                    pieces: vec![straight(2, 100.0)],
                    visible: true,
                },
            ],
            ..Default::default()
        };
        let mut map = Map {
            tiles: vec![Tile::default()],
            ..Default::default()
        };
        let area = Area {
            min: [0.0, 0.0],
            max: [300.0, 300.0],
        };
        let stop = |name: &str, y: f64| StopSpec {
            name: name.into(),
            // ETS 2 z runs south
            outward: [106.0, 300.0 - y],
            back: None,
            rough: false,
        };
        let spec = LineSpec {
            number: "1".into(),
            stops: vec![stop("South", 20.0), stop("North", 180.0)],
            minutes: 5.0,
            vehicle: "bus.bus".into(),
            hof: "hof".into(),
        };
        let report = build(&mut map, &net, &area, &spec);
        assert!(report.iter().all(|l| !l.contains("no way")), "{report:?}");
        let links = &map.timetable.links;
        assert_eq!(links.len(), 2);
        // northwards on the forward lane, back south on the other one
        assert_eq!(
            links[0]
                .steps
                .iter()
                .map(|s| (s.0, s.1))
                .collect::<Vec<_>>(),
            [(1, 2), (2, 2)]
        );
        assert_eq!(
            links[1]
                .steps
                .iter()
                .map(|s| (s.0, s.1))
                .collect::<Vec<_>>(),
            [(2, 1), (1, 1)]
        );
        // the stop of the way back stands opposite the pole
        let objects = &map.tiles[0].objects;
        assert_eq!(objects.len(), 4);
        assert!(
            (objects[2].pos[0] - 94.0).abs() < 0.5,
            "{:?}",
            objects[2].pos
        );
        assert_eq!(map.timetable.trips[1].terminus, "South");
    }
}
