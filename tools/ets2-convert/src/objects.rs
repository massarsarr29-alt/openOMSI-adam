//! Turns ETS 2 models into OMSI scenery objects: for each model, look and variant used on
//! the map, an `.sco` with its mesh (`model\*.o3d`) and textures (`texture\*.dds`).
//!
//! Model space is turned from the game's (x right, y up, forward -z) to the `.o3d` frame,
//! Direct3D's (x right, y up, z forward): `(x, y, z) → (x, y, -z)`. Each triangle is wound
//! so that it faces where its normals face (`(b-a)×(c-a)` along them), which is how OMSI
//! meshes come and what decides which side is drawn.

use crate::scs::model::{tobj_texture, Alpha, Desc, Geometry, Material};
use crate::scs::token::Token;
use crate::convert::{Area, World};
use crate::line::tile_at;
use crate::scs::vfs::Vfs;
use crate::writer::{Map, Object};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

/// The models the map's `def` files name: token → `.pmd` path.
pub fn model_defs(vfs: &Vfs) -> Result<HashMap<Token, String>> {
    let mut out = HashMap::new();
    for name in vfs.list("/def/world")? {
        if !(name.starts_with("model") && name.ends_with(".sii")) {
            continue;
        }
        let Some(d) = vfs.read(&format!("/def/world/{name}"))? else { continue };
        let text = String::from_utf8_lossy(&d);
        for (token, body) in units(&text, "model_def", "model.") {
            if let Some(pmd) = quoted_value(body, "model_desc") {
                out.entry(token).or_insert(pmd);
            }
        }
    }
    Ok(out)
}

/// The `kind : prefix.name { body }` units of a SII text.
fn units<'a>(text: &'a str, kind: &str, prefix: &str) -> Vec<(Token, &'a str)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find(kind) {
        rest = &rest[i + kind.len()..];
        let Some(colon) = rest.find(':') else { break };
        if !rest[..colon].trim().is_empty() {
            continue;
        }
        let after = rest[colon + 1..].trim_start();
        let Some(name) = after.strip_prefix(prefix) else { continue };
        let end = name.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(name.len());
        let (Ok(token), Some(open)) = (name[..end].parse::<Token>(), name.find('{')) else { continue };
        let body = &name[open + 1..];
        let close = body.find("\n}").or_else(|| body.find('}')).unwrap_or(body.len());
        out.push((token, &body[..close]));
        rest = &body[close..];
    }
    out
}

fn quoted_value(body: &str, key: &str) -> Option<String> {
    for line in body.lines() {
        let l = line.trim();
        if let Some(r) = l.strip_prefix(key) {
            let r = r.trim_start();
            if let Some(r) = r.strip_prefix(':') {
                let r = r.trim();
                return Some(r.trim_matches('"').to_string());
            }
        }
    }
    None
}

/// The scenery objects written so far, under `Sceneryobjects\<folder>`.
pub struct Objects<'a> {
    vfs: &'a Vfs,
    /// Folder under `Sceneryobjects`, e.g. `ets2-idf`.
    pub folder: String,
    /// (pmd, look, variant) → the `.sco` path relative to the OMSI folder, or None when the
    /// model could not be made (missing files, nothing visible).
    made: HashMap<(String, Token, Token, [i32; 3]), Option<String>>,
    /// Texture game path → file name in `texture\` (None when it cannot be used).
    textures: HashMap<String, Option<String>>,
    /// Files to write, relative to `Sceneryobjects\<folder>`.
    pub files: BTreeMap<String, Vec<u8>>,
    /// Why models were left out, with how many.
    pub problems: BTreeMap<String, usize>,
}

impl<'a> Objects<'a> {
    pub fn new(vfs: &'a Vfs, folder: &str) -> Objects<'a> {
        Objects {
            vfs,
            folder: folder.into(),
            made: HashMap::new(),
            textures: HashMap::new(),
            files: BTreeMap::new(),
            problems: BTreeMap::new(),
        }
    }

    /// The object for a model in a look and variant, made the first time it is asked for.
    /// OMSI objects have no scale: a model scaled on the map gets its own mesh, with the
    /// scale rounded to 5 % (none within 2.5 % of 1).
    pub fn object(&mut self, pmd: &str, look: Token, variant: Token, scale: [f32; 3]) -> Option<String> {
        let scale = scale.map(|s| if (s - 1.0).abs() < 0.025 { 100 } else { ((s * 20.0).round() * 5.0) as i32 });
        let key = (pmd.to_string(), look, variant, scale);
        if let Some(done) = self.made.get(&key) {
            return done.clone();
        }
        let made = match self.make(pmd, look, variant, scale) {
            Ok(v) => v,
            Err(e) => {
                let first = format!("{e:#}");
                let kind = first.split(':').next().unwrap_or(&first).to_string();
                *self.problems.entry(kind).or_default() += 1;
                None
            }
        };
        self.made.insert(key, made.clone());
        made
    }

    fn make(&mut self, pmd: &str, look: Token, variant: Token, scale: [i32; 3]) -> Result<Option<String>> {
        let sc = scale.map(|s| s as f32 / 100.0);
        let base = pmd.trim_end_matches(".pmd");
        let desc = Desc::parse(&self.vfs.read(pmd)?.context("missing pmd")?).context("bad pmd")?;
        let geo = Geometry::parse(&self.vfs.read(&format!("{base}.pmg"))?.context("missing pmg")?).context("bad pmg")?;
        let li = desc.look_index(look);
        let vi = desc.variant_index(variant);
        let mats = desc.materials.get(li).context("bad pmd")?.clone();

        // one OMSI material per ETS material used
        let mut materials: Vec<(Material, Option<String>)> = Vec::new();
        for m in &mats {
            let mat = match self.vfs.read(m)? {
                Some(t) => Material::parse(&String::from_utf8_lossy(&t)),
                None => {
                    *self.problems.entry("missing mat (part left out)".into()).or_default() += 1;
                    Material { effect: "missing".into(), ..Default::default() }
                }
            };
            let tex = mat.texture.clone().and_then(|t| self.texture(&t));
            materials.push((mat, tex));
        }

        let mut vertices: Vec<[f32; 8]> = Vec::new();
        let mut triangles: Vec<([u32; 3], u16)> = Vec::new();
        // ETS material index → OMSI material index
        let mut used: BTreeMap<usize, u16> = BTreeMap::new();
        let shown = desc.visible.get(vi);
        for (pi, part) in geo.parts.iter().enumerate() {
            if shown.is_some_and(|s| !s.get(pi).copied().unwrap_or(true)) {
                continue;
            }
            for piece in &geo.pieces[part.clone()] {
                let Some((mat, tex)) = materials.get(piece.material) else { continue };
                if mat.invisible() || mat.effect == "missing" {
                    continue;
                }
                // a grey shape where the texture is missing looks worse than nothing
                if tex.is_none() {
                    *self.problems.entry("piece left out, its texture is missing".into()).or_default() += 1;
                    continue;
                }
                let next = used.len() as u16;
                let m = *used.entry(piece.material).or_insert(next);
                let first = vertices.len() as u32;
                for (k, p) in piece.positions.iter().enumerate() {
                    let n = piece.normals.get(k).copied().unwrap_or([0.0, 1.0, 0.0]);
                    let uv = piece.uvs.get(k).copied().unwrap_or([0.0, 0.0]);
                    // a scaled model's normals scale by the inverse
                    let n = [n[0] / sc[0], n[1] / sc[1], n[2] / sc[2]];
                    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-6);
                    vertices.push([p[0] * sc[0], p[1] * sc[1], -p[2] * sc[2], n[0] / l, n[1] / l, -n[2] / l, uv[0], uv[1]]);
                }
                for t in piece.indices.chunks_exact(3) {
                    let t = [first + t[0], first + t[1], first + t[2]];
                    triangles.push((facing_its_normals(&vertices, t), m));
                }
            }
        }
        if triangles.is_empty() {
            return Ok(None);
        }

        let mut name = file_stem(base, look, variant);
        if scale != [100; 3] {
            name.push_str(&format!("-s{}_{}_{}", scale[0], scale[1], scale[2]));
        }
        let mut o3d_mats = Vec::new();
        let mut sco = String::from("File created by ets2-convert\n\n");
        let _ = writeln!(sco, "[mesh]\n{name}.o3d\n");
        for (&ets, _) in &used {
            let (mat, tex) = &materials[ets];
            let tex = tex.clone().unwrap_or_default();
            // [matl] names a texture and which of the materials with that texture it is
            let same = o3d_mats.iter().filter(|(_, t): &&([f32; 3], String)| *t == tex).count();
            o3d_mats.push((mat.diffuse, tex.clone()));
            if !tex.is_empty() {
                let _ = writeln!(sco, "[matl]\n{tex}\n{same}");
                match mat.alpha() {
                    Some(Alpha::Test) => sco.push_str("[matl_alpha]\n1\n"),
                    Some(Alpha::Blend) => sco.push_str("[matl_alpha]\n2\n"),
                    None => {}
                }
                sco.push('\n');
            }
        }
        self.files.insert(format!("model\\{name}.o3d"), o3d(&vertices, &triangles, &o3d_mats));
        self.files.insert(format!("{name}.sco"), sco.into_bytes());
        Ok(Some(format!("Sceneryobjects\\{}\\{name}.sco", self.folder)))
    }

    /// Copies a texture (by its `.tobj`) into `texture\`, once; None when it cannot be used.
    fn texture(&mut self, tobj: &str) -> Option<String> {
        if let Some(t) = self.textures.get(tobj) {
            return t.clone();
        }
        let dds = self
            .vfs
            .read(tobj)
            .ok()
            .flatten()
            .and_then(|d| tobj_texture(&d))
            .or_else(|| Some(tobj.replace(".tobj", ".dds")));
        let out = dds.and_then(|p| {
            let data = self.vfs.read(&p).ok().flatten()?;
            if !usable_dds(&data) {
                *self.problems.entry("texture in a format OMSI cannot read".into()).or_default() += 1;
                return None;
            }
            let name = format!("{}.dds", flat_name(p.trim_end_matches(".dds")));
            self.files.insert(format!("texture\\{name}"), data);
            Some(name)
        });
        if out.is_none() {
            *self.problems.entry("texture not found".into()).or_default() += 1;
        }
        self.textures.insert(tobj.to_string(), out.clone());
        out
    }
}

/// Places the world's model items that stand in the area as objects of the map's tiles,
/// making their objects as they come. Returns a line per thing worth knowing.
pub fn place(map: &mut Map, area: &Area, world: &World, defs: &HashMap<Token, String>, objects: &mut Objects) -> Vec<String> {
    let mut next_id = map.next_id();
    let (mut placed, mut outside, mut no_node, mut not_made, mut waiting) = (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut undefined: BTreeMap<String, usize> = BTreeMap::new();
    for m in &world.models {
        let Some(node) = world.nodes.get(&m.node) else {
            no_node += 1;
            continue;
        };
        if !area.contains(&node.pos) {
            outside += 1;
            continue;
        }
        let Some(pmd) = defs.get(&m.model) else {
            *undefined.entry(m.model.to_string()).or_default() += 1;
            continue;
        };
        // windows and doors are stuck on the walls of buildings, which are not converted yet
        if pmd.contains("window_door_add") {
            waiting += 1;
            continue;
        }
        let Some(file) = objects.object(pmd, m.look, m.variant, m.scale) else {
            not_made += 1;
            continue;
        };
        let p = area.to_omsi(node.pos);
        let (tile, local) = tile_at(map, [p[0], p[1]]);
        let ground = map.tiles[tile].terrain.as_ref().map_or(0.0, |t| t.sample(local[0] as f32, local[1] as f32) as f64);
        let [heading, pitch, bank] = angles(node.rot);
        map.tiles[tile].objects.push(Object {
            file,
            id: next_id,
            pos: [local[0], local[1], p[2] - ground],
            heading,
            tilt: [pitch, bank],
            labels: Vec::new(),
        });
        next_id += 1;
        placed += 1;
    }
    let mut report = vec![format!(
        "models: {placed} placed, {outside} outside the area, {} without a definition ({} kinds), {not_made} that could not be made, {waiting} windows and doors waiting for their buildings, {no_node} without their node",
        undefined.values().sum::<usize>(),
        undefined.len()
    )];
    for (why, n) in &objects.problems {
        report.push(format!("  {why}: {n}"));
    }
    report
}

/// A node's rotation as an OMSI map file's heading, pitch and bank (degrees). The node's
/// quaternion (w, x, y, z) turns the game's frame; in the map's (x east, y north, z up)
/// that is `R = C·Q·Cᵀ` with `C: (x, y, z) → (x, -z, y)`, and Open OMSI builds
/// `Rz(-heading)·Ry(-bank)·Rx(-pitch)` from the file's angles.
fn angles(q: [f32; 4]) -> [f64; 3] {
    let [w, x, y, z] = q.map(f64::from);
    let e = [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ];
    // C·E·Cᵀ: rows and columns (0, 1, 2) of the map frame are (0, -2, 1) of the game's
    let pick = |i: usize| -> (usize, f64) {
        match i {
            0 => (0, 1.0),
            1 => (2, -1.0),
            _ => (1, 1.0),
        }
    };
    let r = |i: usize, j: usize| {
        let ((a, sa), (b, sb)) = (pick(i), pick(j));
        sa * sb * e[a][b]
    };
    let alpha = r(1, 0).atan2(r(0, 0));
    let beta = (-r(2, 0)).clamp(-1.0, 1.0).asin();
    let gamma = r(2, 1).atan2(r(2, 2));
    let tidy = |a: f64| {
        let d = -a.to_degrees();
        if d.abs() < 1e-6 {
            0.0
        } else {
            d
        }
    };
    [(-alpha.to_degrees()).rem_euclid(360.0), tidy(gamma), tidy(beta)]
}

/// The triangle wound so that `(b-a)×(c-a)` points along its vertices' normals.
fn facing_its_normals(v: &[[f32; 8]], t: [u32; 3]) -> [u32; 3] {
    let [a, b, c] = t.map(|i| v[i as usize]);
    let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let g = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
    let n: [f32; 3] = std::array::from_fn(|k| a[3 + k] + b[3 + k] + c[3 + k]);
    if g[0] * n[0] + g[1] * n[1] + g[2] * n[2] < 0.0 {
        [t[0], t[2], t[1]]
    } else {
        t
    }
}

/// The block formats Open OMSI reads: DXT1/3/5, or their DXGI numbers.
fn usable_dds(d: &[u8]) -> bool {
    if d.len() < 128 || &d[0..4] != b"DDS " {
        return false;
    }
    match &d[84..88] {
        b"DXT1" | b"DXT3" | b"DXT5" => true,
        b"DX10" => d.len() >= 148 && matches!(u32::from_le_bytes(d[128..132].try_into().unwrap()), 70..=78),
        _ => false,
    }
}

/// A game path as one file name: `/model/a/b` → `model_a_b`.
fn flat_name(path: &str) -> String {
    path.trim_start_matches('/').replace(['/', '\\', ' ', '.'], "_")
}

fn file_stem(base: &str, look: Token, variant: Token) -> String {
    let mut s = flat_name(base);
    for t in [look, variant] {
        let t = t.to_string();
        if !t.is_empty() && t != "default" {
            s.push('-');
            s.push_str(&t);
        }
    }
    s
}

/// An `.o3d` (version 3, 32-bit indices): vertices of position, normal and uv; triangles
/// with their material; materials of a diffuse colour and a texture.
pub fn o3d(vertices: &[[f32; 8]], triangles: &[([u32; 3], u16)], materials: &[([f32; 3], String)]) -> Vec<u8> {
    let mut b = vec![0x84, 0x19, 3, 1];
    b.push(0x17);
    b.extend((vertices.len() as u32).to_le_bytes());
    for v in vertices {
        for x in v {
            b.extend(x.to_le_bytes());
        }
    }
    b.push(0x49);
    b.extend((triangles.len() as u32).to_le_bytes());
    for (t, m) in triangles {
        for i in t {
            b.extend(i.to_le_bytes());
        }
        b.extend(m.to_le_bytes());
    }
    b.push(0x26);
    b.extend((materials.len() as u16).to_le_bytes());
    for (d, tex) in materials {
        for x in [d[0], d[1], d[2], 1.0, 0.1, 0.1, 0.1, 0.0, 0.0, 0.0, 10.0] {
            b.extend(x.to_le_bytes());
        }
        let t = tex.as_bytes();
        b.push(t.len().min(255) as u8);
        b.extend(&t[..t.len().min(255)]);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_of_a_sii() {
        let text = "SiiNunit\n{\nmodel_def : model.idf41 {\n model_desc: \"/asset/shapes.pmd\"\n name: \"x\"\n}\nmodel_def : model.9998\n{\nmodel_desc : \"/a/b.pmd\"\n}\n}";
        let u = units(text, "model_def", "model.");
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].0.to_string(), "idf41");
        assert_eq!(quoted_value(u[0].1, "model_desc").as_deref(), Some("/asset/shapes.pmd"));
        assert_eq!(quoted_value(u[1].1, "model_desc").as_deref(), Some("/a/b.pmd"));
    }

    #[test]
    fn angles_of_a_node() {
        // no turn: facing -z in the game, north on the map
        assert_eq!(angles([1.0, 0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        // a quarter turn about the game's up axis (y), anticlockwise seen from above: west
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let a = angles([h, 0.0, h, 0.0]);
        assert!((a[0] - 270.0).abs() < 1e-4 && a[1].abs() < 1e-4 && a[2].abs() < 1e-4, "{a:?}");
        // a tilted node: Open OMSI's Rz(-heading)·Ry(-bank)·Rx(-pitch) turns the model's
        // forward axis (map y) where the node points
        let l = (0.81f32 + 0.01 + 0.09 + 0.0025).sqrt();
        let n = crate::scs::sector::Node { uid: 0, pos: [0.0; 3], rot: [0.9, 0.1, 0.3, 0.05].map(|v| v / l), backward_item: 0, forward_item: 0, flags: 0 };
        let d = crate::convert::direction(&n);
        let [h, p, b] = angles(n.rot).map(|a| (-a).to_radians());
        let (rz, ry, rx) = (h, b, p);
        // Rx(rx)·(0, 1, 0), then Ry(ry), then Rz(rz)
        let v = [0.0, rx.cos(), rx.sin()];
        let v = [v[0] * ry.cos() + v[2] * ry.sin(), v[1], -v[0] * ry.sin() + v[2] * ry.cos()];
        let v = [v[0] * rz.cos() - v[1] * rz.sin(), v[0] * rz.sin() + v[1] * rz.cos(), v[2]];
        assert!((0..3).all(|k| (v[k] - d[k]).abs() < 1e-5), "{v:?} {d:?}");
    }

    #[test]
    fn o3d_reads_back() {
        let v = [[0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0], [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0], [0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0]];
        let b = o3d(&v, &[([0, 1, 2], 0)], &[([1.0, 1.0, 1.0], "a.dds".into())]);
        let m = omsi_o3d::parse_o3d(&b).unwrap();
        assert_eq!((m.vertices.len(), m.triangles.len(), m.materials[0].texture.as_str()), (3, 1, "a.dds"));
    }
}
