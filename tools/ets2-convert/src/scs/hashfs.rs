//! HashFS version 2 archives (the game's `.scs` files since 1.50): entries are found by the
//! CityHash64 (version 1.0.3) of their path without the leading slash. Only plain files and
//! directory listings are read; packed textures are left out.

use anyhow::{bail, ensure, Context, Result};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub struct HashFs {
    file: std::sync::Mutex<std::fs::File>,
    /// hash → (metadata index, metadata count, flags)
    entries: HashMap<u64, (u32, u16, u16)>,
    meta: Vec<u32>,
}

const PLAIN: u32 = 0x80;
const DIRECTORY: u32 = 0x81;

impl HashFs {
    pub fn open(path: &Path) -> Result<HashFs> {
        let mut f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let mut h = [0u8; 64];
        f.read_exact(&mut h)?;
        ensure!(&h[0..4] == b"SCS#", "{} is not a HashFS archive", path.display());
        let u16at = |i: usize| u16::from_le_bytes(h[i..i + 2].try_into().unwrap());
        let u32at = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
        let u64at = |i: usize| u64::from_le_bytes(h[i..i + 8].try_into().unwrap());
        let version = u16at(4);
        ensure!(version == 2, "HashFS version {version} (this reader knows 2)");
        ensure!(u16at(6) == 0, "salted HashFS archives are not supported");
        ensure!(&h[8..12] == b"CITY", "unknown hash method");
        let count = u32at(12) as usize;
        let table_size = u32at(16) as usize;
        let meta_count = u32at(20) as usize;
        let meta_size = u32at(24) as usize;
        let table_at = u64at(28);
        let meta_at = u64at(36);
        let table = inflate(&read_at(&mut f, table_at, table_size)?)?;
        let meta_bytes = inflate(&read_at(&mut f, meta_at, meta_size)?)?;
        ensure!(table.len() >= count * 16 && meta_bytes.len() >= meta_count * 4, "short tables");
        let mut entries = HashMap::with_capacity(count);
        for e in table.chunks_exact(16).take(count) {
            entries.insert(
                u64::from_le_bytes(e[0..8].try_into().unwrap()),
                (
                    u32::from_le_bytes(e[8..12].try_into().unwrap()),
                    u16::from_le_bytes(e[12..14].try_into().unwrap()),
                    u16::from_le_bytes(e[14..16].try_into().unwrap()),
                ),
            );
        }
        let meta = meta_bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect();
        Ok(HashFs {
            file: std::sync::Mutex::new(f),
            entries,
            meta,
        })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&city_hash64(path.trim_start_matches('/').as_bytes()))
    }

    /// A plain file or directory listing: its bytes, and whether it is a directory.
    fn raw(&self, path: &str) -> Result<Option<(Vec<u8>, bool)>> {
        let Some(&(index, n, _)) = self.entries.get(&city_hash64(path.trim_start_matches('/').as_bytes())) else {
            return Ok(None);
        };
        for k in 0..n as usize {
            let head = *self.meta.get(index as usize + k).context("metadata index")?;
            let (at, ty) = ((head & 0xff_ffff) as usize, head >> 24);
            if ty != PLAIN && ty != DIRECTORY {
                continue;
            }
            let m = self.meta.get(at..at + 4).context("metadata entry")?;
            let packed = (m[0] & 0x0fff_ffff) as usize;
            let compression = m[0] >> 28;
            let size = (m[1] & 0x0fff_ffff) as usize;
            let offset = m[3] as u64 * 16;
            let bytes = read_at(&mut self.file.lock().unwrap(), offset, packed)?;
            let bytes = match compression {
                0 => bytes,
                1 => inflate(&bytes)?,
                c => bail!("{path}: compression {c} is not supported"),
            };
            ensure!(bytes.len() == size, "{path}: {} bytes instead of {size}", bytes.len());
            return Ok(Some((bytes, ty == DIRECTORY)));
        }
        Ok(None)
    }

    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.raw(path)?.filter(|(_, dir)| !dir).map(|(b, _)| b))
    }

    /// Names in a directory; sub-directories start with `/`.
    pub fn list(&self, path: &str) -> Result<Vec<String>> {
        let Some((d, true)) = self.raw(path)? else { return Ok(Vec::new()) };
        let n = u32::from_le_bytes(d.get(0..4).context("listing")?.try_into().unwrap()) as usize;
        let lens = d.get(4..4 + n).context("listing")?;
        let mut at = 4 + n;
        let mut out = Vec::with_capacity(n);
        for &l in lens {
            let name = d.get(at..at + l as usize).context("listing")?;
            out.push(String::from_utf8_lossy(name).into_owned());
            at += l as usize;
        }
        Ok(out)
    }
}

fn read_at(f: &mut std::fs::File, at: u64, n: usize) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(at))?;
    let mut b = vec![0; n];
    f.read_exact(&mut b)?;
    Ok(b)
}

fn inflate(b: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    flate2::read::ZlibDecoder::new(b).read_to_end(&mut out)?;
    Ok(out)
}

const K0: u64 = 0xc3a5c85c97cb3127;
const K1: u64 = 0xb492b66fbe98f273;
const K2: u64 = 0x9ae16a3b2f90404f;
const K3: u64 = 0xc949d7c7509e6557;

fn fetch64(s: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(s[i..i + 8].try_into().unwrap())
}

fn fetch32(s: &[u8], i: usize) -> u64 {
    u32::from_le_bytes(s[i..i + 4].try_into().unwrap()) as u64
}

fn shift_mix(v: u64) -> u64 {
    v ^ (v >> 47)
}

fn hash16(u: u64, v: u64) -> u64 {
    const MUL: u64 = 0x9ddfea08eb382d69;
    let mut a = (u ^ v).wrapping_mul(MUL);
    a ^= a >> 47;
    let mut b = (v ^ a).wrapping_mul(MUL);
    b ^= b >> 47;
    b.wrapping_mul(MUL)
}

fn len0to16(s: &[u8]) -> u64 {
    let n = s.len();
    if n > 8 {
        let a = fetch64(s, 0);
        let b = fetch64(s, n - 8);
        return hash16(a, b.wrapping_add(n as u64).rotate_right(n as u32)) ^ b;
    }
    if n >= 4 {
        let a = fetch32(s, 0);
        return hash16((n as u64).wrapping_add(a << 3), fetch32(s, n - 4));
    }
    if n > 0 {
        let (a, b, c) = (s[0] as u32, s[n >> 1] as u32, s[n - 1] as u32);
        let y = a.wrapping_add(b << 8);
        let z = (n as u32).wrapping_add(c << 2);
        return shift_mix((y as u64).wrapping_mul(K2) ^ (z as u64).wrapping_mul(K3)).wrapping_mul(K2);
    }
    K2
}

fn len17to32(s: &[u8]) -> u64 {
    let n = s.len();
    let a = fetch64(s, 0).wrapping_mul(K1);
    let b = fetch64(s, 8);
    let c = fetch64(s, n - 8).wrapping_mul(K2);
    let d = fetch64(s, n - 16).wrapping_mul(K0);
    hash16(
        a.wrapping_sub(b).rotate_right(43).wrapping_add(c.rotate_right(30)).wrapping_add(d),
        a.wrapping_add((b ^ K3).rotate_right(20)).wrapping_sub(c).wrapping_add(n as u64),
    )
}

fn len33to64(s: &[u8]) -> u64 {
    let n = s.len();
    let mut z = fetch64(s, 24);
    let mut a = fetch64(s, 0).wrapping_add((n as u64).wrapping_add(fetch64(s, n - 16)).wrapping_mul(K0));
    let mut b = a.wrapping_add(z).rotate_right(52);
    let mut c = a.rotate_right(37);
    a = a.wrapping_add(fetch64(s, 8));
    c = c.wrapping_add(a.rotate_right(7));
    a = a.wrapping_add(fetch64(s, 16));
    let vf = a.wrapping_add(z);
    let vs = b.wrapping_add(a.rotate_right(31)).wrapping_add(c);
    a = fetch64(s, 16).wrapping_add(fetch64(s, n - 32));
    z = fetch64(s, n - 8);
    b = a.wrapping_add(z).rotate_right(52);
    c = a.rotate_right(37);
    a = a.wrapping_add(fetch64(s, n - 24));
    c = c.wrapping_add(a.rotate_right(7));
    a = a.wrapping_add(fetch64(s, n - 16));
    let wf = a.wrapping_add(z);
    let ws = b.wrapping_add(a.rotate_right(31)).wrapping_add(c);
    let r = shift_mix(vf.wrapping_add(ws).wrapping_mul(K2).wrapping_add(wf.wrapping_add(vs).wrapping_mul(K0)));
    shift_mix(r.wrapping_mul(K0).wrapping_add(vs)).wrapping_mul(K2)
}

fn weak32(s: &[u8], i: usize, mut a: u64, mut b: u64) -> (u64, u64) {
    let (w, x, y, z) = (fetch64(s, i), fetch64(s, i + 8), fetch64(s, i + 16), fetch64(s, i + 24));
    a = a.wrapping_add(w);
    b = b.wrapping_add(a).wrapping_add(z).rotate_right(21);
    let c = a;
    a = a.wrapping_add(x).wrapping_add(y);
    b = b.wrapping_add(a.rotate_right(44));
    (a.wrapping_add(z), b.wrapping_add(c))
}

/// CityHash64 as of version 1.0.3, which the archives use.
pub fn city_hash64(s: &[u8]) -> u64 {
    let n = s.len();
    if n <= 16 {
        return len0to16(s);
    }
    if n <= 32 {
        return len17to32(s);
    }
    if n <= 64 {
        return len33to64(s);
    }
    let mut x = fetch64(s, n - 40);
    let mut y = fetch64(s, n - 16).wrapping_add(fetch64(s, n - 56));
    let mut z = hash16(fetch64(s, n - 48).wrapping_add(n as u64), fetch64(s, n - 24));
    let mut v = weak32(s, n - 64, n as u64, z);
    let mut w = weak32(s, n - 32, y.wrapping_add(K1), x);
    x = x.wrapping_mul(K1).wrapping_add(fetch64(s, 0));
    let mut left = (n - 1) & !63;
    let mut i = 0;
    loop {
        x = x.wrapping_add(y).wrapping_add(v.0).wrapping_add(fetch64(s, i + 8)).rotate_right(37).wrapping_mul(K1);
        y = y.wrapping_add(v.1).wrapping_add(fetch64(s, i + 48)).rotate_right(42).wrapping_mul(K1);
        x ^= w.1;
        y = y.wrapping_add(v.0).wrapping_add(fetch64(s, i + 40));
        z = z.wrapping_add(w.0).rotate_right(33).wrapping_mul(K1);
        v = weak32(s, i, v.1.wrapping_mul(K1), x.wrapping_add(w.0));
        w = weak32(s, i + 32, z.wrapping_add(w.1), y.wrapping_add(fetch64(s, i + 16)));
        std::mem::swap(&mut z, &mut x);
        i += 64;
        left -= 64;
        if left == 0 {
            break;
        }
    }
    hash16(
        hash16(v.0, w.0).wrapping_add(shift_mix(y).wrapping_mul(K1)).wrapping_add(z),
        hash16(v.1, w.1).wrapping_add(x),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_match_the_archives() {
        // paths found in the game's def.scs, one of each length class
        for (path, hash) in [
            ("", K2),
            ("def", 0x2c6f469efb31c45a),
            ("custom", 0xe31afc07be2b38fb),
            ("def/world", 0x8b419af4bcde2c23),
            ("def/city.sii", 0x9a5cb1ebaac98229),
            ("def/world/prefab.sii", 0x89673e3ca7416e05),
            ("def/world/prefab.base_share.sii", 0x727b182e6434f431),
            ("def/vehicle/truck_company_uk/oversize/scania_streamline_8x4_top_730.sii", 0xf6d491990d92e514),
        ] {
            assert_eq!(city_hash64(path.as_bytes()), hash, "{path}");
        }
    }
}
