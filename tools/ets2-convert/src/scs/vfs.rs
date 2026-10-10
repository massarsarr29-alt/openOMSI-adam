//! The game's file system as the game sees it: archives and folders stacked, the first
//! that has a path wins (a mod before the game's own archives).

use super::hashfs::HashFs;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

enum Source {
    Dir(PathBuf),
    Zip {
        zip: Mutex<zip::ZipArchive<std::fs::File>>,
        /// lower-case path without the leading slash → index in the archive
        names: HashMap<String, usize>,
    },
    HashFs(HashFs),
}

#[derive(Default)]
pub struct Vfs {
    sources: Vec<(String, Source)>,
}

impl Vfs {
    /// Adds a folder, a ZIP archive (a mod's `.scs` or a zipped extraction) or a HashFS
    /// archive, behind the ones added before.
    pub fn add(&mut self, path: &Path) -> Result<()> {
        let label = path.display().to_string();
        let source = if path.is_dir() {
            Source::Dir(path.to_path_buf())
        } else {
            let mut head = [0u8; 4];
            std::fs::File::open(path)
                .with_context(|| format!("opening {label}"))?
                .read_exact(&mut head)?;
            if &head == b"SCS#" {
                Source::HashFs(HashFs::open(path)?)
            } else {
                let zip = zip::ZipArchive::new(std::fs::File::open(path)?).with_context(|| format!("reading {label}"))?;
                let names = (0..zip.len())
                    .filter_map(|i| zip.name_for_index(i).map(|n| (n.trim_start_matches('/').to_ascii_lowercase(), i)))
                    .collect();
                Source::Zip {
                    zip: Mutex::new(zip),
                    names,
                }
            }
        };
        self.sources.push((label, source));
        Ok(())
    }

    /// A file by its game path (`/model/a/b.pmd`).
    pub fn read(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let rel = path.trim_start_matches('/');
        for (label, s) in &self.sources {
            let found = match s {
                Source::Dir(d) => {
                    let p = d.join(rel);
                    p.is_file().then(|| std::fs::read(&p)).transpose()?
                }
                Source::Zip { zip, names } => match names.get(&rel.to_ascii_lowercase()) {
                    Some(&i) => {
                        let mut z = zip.lock().unwrap();
                        let mut f = z.by_index(i)?;
                        let mut b = Vec::with_capacity(f.size() as usize);
                        f.read_to_end(&mut b)?;
                        Some(b)
                    }
                    None => None,
                },
                Source::HashFs(h) => h.read(rel).with_context(|| format!("reading {path} from {label}"))?,
            };
            if found.is_some() {
                return Ok(found);
            }
        }
        Ok(None)
    }

    /// File names in a directory, merged over every source (sub-directories left out).
    pub fn list(&self, dir: &str) -> Result<Vec<String>> {
        let rel = dir.trim_matches('/');
        let mut out: Vec<String> = Vec::new();
        for (_, s) in &self.sources {
            match s {
                Source::Dir(d) => {
                    if let Ok(rd) = std::fs::read_dir(d.join(rel)) {
                        for e in rd.flatten() {
                            if e.path().is_file() {
                                out.push(e.file_name().to_string_lossy().into_owned());
                            }
                        }
                    }
                }
                Source::Zip { names, .. } => {
                    let prefix = format!("{}/", rel.to_ascii_lowercase());
                    for n in names.keys() {
                        if let Some(rest) = n.strip_prefix(&prefix) {
                            if !rest.contains('/') && !rest.is_empty() {
                                out.push(rest.to_string());
                            }
                        }
                    }
                }
                Source::HashFs(h) => out.extend(h.list(rel)?.into_iter().filter(|n| !n.starts_with('/'))),
            }
        }
        out.sort();
        out.dedup();
        Ok(out)
    }
}
