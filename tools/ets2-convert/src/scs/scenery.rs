//! A sector's `.aux` file ("scenery" here: `aux` cannot name a file on Windows) holds
//! the items that are only scenery to the game, among them the model items (type 5):
//! street furniture such as bus stop poles. Their layout after the common item header is
//! the model token, its look, its variant, a list of additional parts and the node the
//! model stands on. The file is not walked item by item here: model items of the wanted
//! models are found by their token preceded, 57 bytes earlier, by type 5, and the node's
//! position is read from the node list at the end of the file.

use super::token::Token;

/// A model standing on the map.
#[derive(Debug, Clone)]
pub struct ModelItem {
    pub model: Token,
    pub look: Token,
    pub variant: Token,
    /// Metres, global: x east, y up, z south.
    pub pos: [f64; 3],
}

fn u32at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

fn u64at(d: &[u8], i: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(i..i + 8)?.try_into().ok()?))
}

fn i32at(d: &[u8], i: usize) -> Option<i32> {
    Some(i32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

/// The model items of the `wanted` models in an `.aux` file.
pub fn models(d: &[u8], wanted: &[Token]) -> Vec<ModelItem> {
    let mut out = Vec::new();
    for i in 57..d.len().saturating_sub(8) {
        let Some(t) = u64at(d, i) else { break };
        if !wanted.iter().any(|w| w.0 == t) || u32at(d, i - 57) != Some(5) {
            continue;
        }
        let item = || -> Option<ModelItem> {
            let parts = u32at(d, i + 24)? as usize;
            let at = i + 28 + parts * 8;
            let node = u64at(d, at)?.to_le_bytes();
            // the node's own record is the last place its uid appears
            let k = d.windows(8).rposition(|w| w == node)?;
            if k == at {
                return None;
            }
            let p = |o: usize| i32at(d, k + o).map(|v| v as f64 / 256.0);
            Some(ModelItem {
                model: Token(t),
                look: Token(u64at(d, i + 8)?),
                variant: Token(u64at(d, i + 16)?),
                pos: [p(8)?, p(12)?, p(16)?],
            })
        };
        out.extend(item());
    }
    out
}
