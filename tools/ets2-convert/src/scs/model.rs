//! ETS 2 models: the `.pmd` description (materials per look, which parts each variant
//! shows) and the `.pmg` geometry (pieces of triangles grouped into parts), plus the
//! `.mat` materials and `.tobj` texture objects they point to.
//!
//! Coordinates are the model's own: x right, y up, z towards the back (forward is -z).

use super::reader::Reader;
use super::token::Token;
use anyhow::{bail, ensure, Context, Result};

/// A `.pmd` (version 4).
#[derive(Debug, Clone, Default)]
pub struct Desc {
    /// Material paths of each look, `materials[look][index]`.
    pub materials: Vec<Vec<String>>,
    pub looks: Vec<Token>,
    pub variants: Vec<Token>,
    /// For each variant, whether each part is shown.
    pub visible: Vec<Vec<bool>>,
}

impl Desc {
    pub fn parse(d: &[u8]) -> Result<Desc> {
        let mut r = Reader::new(d);
        let version = r.u32()?;
        ensure!(version == 4, "pmd version {version} (this reader knows 4)");
        let materials = r.u32()? as usize;
        let looks = r.u32()? as usize;
        r.u32()?; // pieces
        let variants = r.u32()? as usize;
        let parts = r.u32()? as usize;
        let attributes = r.u32()? as usize;
        let values_size = r.u32()? as usize;
        r.u32()?; // material block size
        let look_at = r.u32()? as usize;
        let variant_at = r.u32()? as usize;
        let part_attr_at = r.u32()? as usize;
        let values_at = r.u32()? as usize;
        let attr_at = r.u32()? as usize;
        let material_at = r.u32()? as usize;
        r.u32()?; // material data

        let tokens = |at: usize, n: usize| -> Result<Vec<Token>> {
            let mut r = Reader::new(d);
            r.pos = at;
            (0..n).map(|_| r.token()).collect()
        };
        let look_names = tokens(look_at, looks)?;
        let variant_names = tokens(variant_at, variants)?;

        let mut r = Reader::new(d);
        r.pos = attr_at;
        let mut attrs = Vec::with_capacity(attributes);
        for _ in 0..attributes {
            let name = r.token()?;
            let ty = r.i32()?;
            let offset = r.u32()? as usize;
            attrs.push((name, ty, offset));
        }
        r.pos = part_attr_at;
        let mut part_attrs = Vec::with_capacity(parts);
        for _ in 0..parts {
            // the part's attributes, as first and one past the last
            let from = r.u32()? as usize;
            let to = r.u32()? as usize;
            part_attrs.push(from..to.max(from));
        }
        let visible_name: Token = "visible".parse().unwrap();
        let mut visible = Vec::with_capacity(variants);
        for v in 0..variants {
            let row = part_attrs
                .iter()
                .map(|range| {
                    range.clone().all(|a| match attrs.get(a) {
                        Some(&(name, _, offset)) if name == visible_name => {
                            let at = values_at + v * values_size + offset;
                            d.get(at..at + 4).map_or(true, |b| u32::from_le_bytes(b.try_into().unwrap()) != 0)
                        }
                        _ => true,
                    })
                })
                .collect();
            visible.push(row);
        }

        let mut r = Reader::new(d);
        r.pos = material_at;
        let mut mats = Vec::with_capacity(looks);
        for _ in 0..looks {
            let mut row = Vec::with_capacity(materials);
            for _ in 0..materials {
                let at = r.u32()? as usize;
                row.push(c_string(d, at)?);
            }
            mats.push(row);
        }
        Ok(Desc {
            materials: mats,
            looks: look_names,
            variants: variant_names,
            visible,
        })
    }

    pub fn look_index(&self, look: Token) -> usize {
        self.looks.iter().position(|&l| l == look).unwrap_or(0)
    }

    pub fn variant_index(&self, variant: Token) -> usize {
        self.variants.iter().position(|&v| v == variant).unwrap_or(0)
    }
}

fn c_string(d: &[u8], at: usize) -> Result<String> {
    let rest = d.get(at..).context("string past the end")?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// One piece of a `.pmg`: triangles of one material.
#[derive(Debug, Clone, Default)]
pub struct Piece {
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// First texture coordinates, when the piece has any.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// A `.pmg` (version 0x15): pieces, and which pieces form each part.
#[derive(Debug, Clone, Default)]
pub struct Geometry {
    pub pieces: Vec<Piece>,
    /// Piece ranges of each part, in the part order of the `.pmd`.
    pub parts: Vec<std::ops::Range<usize>>,
}

impl Geometry {
    pub fn parse(d: &[u8]) -> Result<Geometry> {
        let mut r = Reader::new(d);
        let magic = r.u32()?;
        ensure!(magic & 0xff == 0x15 && magic >> 8 == 0x506d67, "not a pmg of version 0x15 ({magic:08x})");
        let pieces = r.u32()? as usize;
        let parts = r.u32()? as usize;
        r.skip(4 + 4 + 4 + 8)?; // bones, weight width, locators, skeleton hash
        r.skip(4 * 10)?; // bounding sphere and box
        r.u32()?; // skeleton
        let parts_at = r.u32()? as usize;
        r.u32()?; // locators
        let pieces_at = r.u32()? as usize;

        let mut r = Reader::new(d);
        r.pos = parts_at;
        let mut part_ranges = Vec::with_capacity(parts);
        for _ in 0..parts {
            r.token()?;
            let n = r.u32()? as usize;
            let from = r.u32()? as usize;
            r.skip(8)?; // locators
            part_ranges.push(from..from + n);
        }

        r.pos = pieces_at;
        let mut out = Vec::with_capacity(pieces);
        for _ in 0..pieces {
            let edges = r.u32()? as usize;
            let verts = r.u32()? as usize;
            let uv_mask = r.u32()?;
            let uv_count = r.u32()? as usize;
            let material = r.i32()?.max(0) as usize;
            r.skip(4 * 10)?; // bounds
            let stride = r.u32()? as usize;
            let pos_at = r.i32()?;
            let normal_at = r.i32()?;
            let uv_at = r.i32()?;
            r.skip(4 * 5)?; // colours, tangent, bone indices and weights
            let index_at = r.i32()?;
            ensure!(pos_at >= 0 && index_at >= 0, "piece without positions or indices");
            let vec3 = |at: usize| -> Result<[f32; 3]> {
                let b = d.get(at..at + 12).context("vertex past the end")?;
                Ok(std::array::from_fn(|k| f32::from_le_bytes(b[k * 4..k * 4 + 4].try_into().unwrap())))
            };
            let mut p = Piece {
                material,
                ..Default::default()
            };
            for v in 0..verts {
                let base = v * stride;
                p.positions.push(vec3(pos_at as usize + base)?);
                if normal_at >= 0 {
                    p.normals.push(vec3(normal_at as usize + base)?);
                }
                if uv_at >= 0 && uv_count > 0 {
                    // the mask names, per texture coordinate slot, which stored pair it uses;
                    // slot 0 is the one the base texture reads
                    let slot = (uv_mask & 0xf) as usize;
                    let slot = if slot < uv_count { slot } else { 0 };
                    let at = uv_at as usize + base + slot * 8;
                    let b = d.get(at..at + 8).context("uv past the end")?;
                    p.uvs.push([
                        f32::from_le_bytes(b[0..4].try_into().unwrap()),
                        f32::from_le_bytes(b[4..8].try_into().unwrap()),
                    ]);
                }
            }
            for k in 0..edges {
                let at = index_at as usize + k * 2;
                let b = d.get(at..at + 2).context("index past the end")?;
                let i = u16::from_le_bytes(b.try_into().unwrap()) as u32;
                if i as usize >= verts {
                    bail!("index {i} of a piece with {verts} vertices");
                }
                p.indices.push(i);
            }
            out.push(p);
        }
        Ok(Geometry {
            pieces: out,
            parts: part_ranges,
        })
    }
}

/// What the conversion needs of a `.mat`: its effect and base texture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Material {
    pub effect: String,
    /// The `.tobj` of the base texture, when there is one.
    pub texture: Option<String>,
    /// The `.tobj` the base texture is multiplied by (`.mult.` effects), when there is one.
    pub mult: Option<String>,
    pub diffuse: [f32; 3],
}

impl Material {
    pub fn parse(text: &str) -> Material {
        let quoted = |after: &str| -> Option<String> {
            let i = text.find(after)? + after.len();
            let rest = &text[i..];
            let a = rest.find('"')? + 1;
            let b = rest[a..].find('"')? + a;
            Some(rest[a..b].to_string())
        };
        let effect = quoted("effect").unwrap_or_default();
        // newer files name each texture in a block, `texture : "texture_base" { source : ".." }`;
        // older ones list them, `texture[0] : ".."`
        let texture = quoted("source")
            .filter(|s| s.ends_with(".tobj"))
            .or_else(|| quoted("texture[0]"))
            .or_else(|| quoted("texture :").filter(|s| s.ends_with(".tobj")));
        let mut diffuse = [1.0; 3];
        if let Some(i) = text.find("diffuse") {
            let rest = &text[i..];
            if let (Some(a), Some(b)) = (rest.find('{'), rest.find('}')) {
                let v: Vec<f32> = rest[a + 1..b].split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if v.len() >= 3 {
                    diffuse = [v[0], v[1], v[2]];
                }
            }
        }
        let mult = text.find("\"texture_mult\"").and_then(|i| {
            let rest = &text[i..];
            let a = rest.find("source")?;
            let rest = &rest[a..];
            let q = rest.find('"')? + 1;
            let e = rest[q..].find('"')? + q;
            Some(rest[q..e].to_string())
        });
        Material { effect, texture, mult, diffuse }
    }

    /// Effects that draw nothing seen in daylight, or that OMSI cannot draw the way the game
    /// does: shadow casters, collision, flares, night lights, additive glows (their black
    /// ground would show) and glass (an opaque dark pane over the window behind it).
    pub fn invisible(&self) -> bool {
        let e = &self.effect;
        e.contains("shadowonly")
            || e.contains("none.rfx")
            || e.contains("flare")
            || e.contains(".night")
            || e.contains(".add.rfx")
            || e.contains("glass")
            || e.is_empty()
    }

    /// The texture has an alpha channel to cut out (`.a.` effects) or blend (`.over`).
    pub fn alpha(&self) -> Option<Alpha> {
        let e = &self.effect;
        if e.contains(".a.over") || e.contains(".over.") || e.contains("decal") || e.contains("glass") {
            Some(Alpha::Blend)
        } else if e.contains(".a.") || e.ends_with(".a.rfx") {
            Some(Alpha::Test)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alpha {
    Test,
    Blend,
}

/// The `.dds` a `.tobj` points to: after a 40-byte header, the path's length and the path.
pub fn tobj_texture(d: &[u8]) -> Option<String> {
    let n = u64::from_le_bytes(d.get(40..48)?.try_into().ok()?) as usize;
    let path = d.get(48..48 + n)?;
    Some(String::from_utf8_lossy(path).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_with_a_texture_block() {
        let m = Material::parse(
            "effect : \"eut2.dif.spec.a.rfx\" {\n\tdiffuse : { 0.5 , 1.0 , 1.0 }\n\ttexture : \"texture_base\" {\n\t\tsource : \"/model/a/b.tobj\"\n\t}\n}",
        );
        assert_eq!(m.effect, "eut2.dif.spec.a.rfx");
        assert_eq!(m.texture.as_deref(), Some("/model/a/b.tobj"));
        assert_eq!(m.diffuse, [0.5, 1.0, 1.0]);
        assert_eq!(m.alpha(), Some(Alpha::Test));
        assert_eq!(m.mult, None);
        let gs = Material::parse(
            "effect : \"eut2.dif.spec.mult.dif.spec.iamod.dif.spec.rfx\" {\n\ttexture : \"texture_base\" {\n\t\tsource : \"/material/tiled/plaster_gs_01.tobj\"\n\t}\n\ttexture : \"texture_mult\" {\n\t\tsource : \"/material/tiled/plaster_over_01.tobj\"\n\t}\n}",
        );
        assert_eq!(gs.texture.as_deref(), Some("/material/tiled/plaster_gs_01.tobj"));
        assert_eq!(gs.mult.as_deref(), Some("/material/tiled/plaster_over_01.tobj"));
        let old = Material::parse("material : \"eut2.dif\" {\n\ttexture[0] : \"/x/y.tobj\"\n}");
        assert_eq!(old.texture.as_deref(), Some("/x/y.tobj"));
    }

    #[test]
    fn tobj_names_its_dds() {
        let mut d = vec![1u8, 10, 177, 112];
        d.extend([0u8; 36]);
        d.extend(44u64.to_le_bytes());
        d.extend(b"/model/dyx/signs/traffic/traffic_base_01.dds");
        assert_eq!(tobj_texture(&d).as_deref(), Some("/model/dyx/signs/traffic/traffic_base_01.dds"));
    }
}
