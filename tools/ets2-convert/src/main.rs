use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Converts Euro Truck Simulator 2 maps to OMSI 2 maps")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Writes a one-tile test map (a road and a curve) into a folder, e.g.
    /// "<OMSI 2>/maps/ets2-convert-test".
    TestMap { out: PathBuf },
    /// Reads every sector of a map mod (.scs/.zip, or a folder with its map folder) and says
    /// what it holds.
    Inspect { archive: PathBuf },
    /// Converts the roads around an ETS 2 city into an OMSI map folder.
    Convert {
        archive: PathBuf,
        out: PathBuf,
        /// City name as the map defines it, e.g. "vincennes".
        #[arg(long)]
        city: String,
        /// Width of the converted square, in 300 m tiles.
        #[arg(long, default_value_t = 5)]
        tiles: u32,
        /// The OMSI spline every road is drawn with.
        #[arg(
            long,
            default_value = "Splines\\Marcel\\str_2spur_11m_SeeburgerStr1.sli"
        )]
        spline: String,
        /// Map name shown in OMSI (defaults to the out folder name).
        #[arg(long)]
        name: Option<String>,
    },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::TestMap { out } => {
            ets2_convert::test_map::build().write(&out)?;
            println!("test map written to {}", out.display());
        }
        Command::Inspect { archive } => inspect(&archive)?,
        Command::Convert {
            archive,
            out,
            city,
            tiles,
            spline,
            name,
        } => {
            use ets2_convert::convert::{convert, Area, Options, World};
            let mut world = World::default();
            for (n, data) in sector_files(&archive)? {
                let s = ets2_convert::scs::sector::Sector::parse_base(&data)
                    .with_context(|| format!("reading {n}"))?;
                world.add(s);
            }
            let centre = world
                .city_centre(&city)
                .with_context(|| format!("no city called {city} in the map"))?;
            let name = name.unwrap_or_else(|| {
                out.file_name()
                    .map_or("ets2".into(), |n| n.to_string_lossy().into_owned())
            });
            let map = convert(
                &world,
                &Options {
                    name,
                    area: Area::around(centre, tiles),
                    road_spline: spline,
                },
            );
            let splines: usize = map.tiles.iter().map(|t| t.splines.len()).sum();
            map.write(&out)?;
            println!(
                "{} tiles, {splines} splines written to {}",
                map.tiles.len(),
                out.display()
            );
        }
    }
    Ok(())
}

/// The .base files of a map mod archive, or of a folder its "map" folder was copied to.
fn sector_files(archive: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>> {
    use std::io::Read;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if archive.is_dir() {
        let mut stack = vec![archive.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "base") {
                    files.push((p.display().to_string(), std::fs::read(&p)?));
                }
            }
        }
    } else {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
        let names: Vec<String> = zip
            .file_names()
            .filter(|n| n.starts_with("map/") && n.ends_with(".base"))
            .map(String::from)
            .collect();
        for n in names {
            let mut data = Vec::new();
            zip.by_name(&n)?.read_to_end(&mut data)?;
            files.push((n, data));
        }
    }
    files.sort();
    Ok(files)
}

fn inspect(archive: &std::path::Path) -> Result<()> {
    use ets2_convert::scs::sector::Sector;
    use std::collections::BTreeMap;
    let files = sector_files(archive)?;
    let (mut ok, mut failed) = (0, 0);
    let mut types: BTreeMap<u32, usize> = BTreeMap::new();
    let (mut roads, mut prefabs, mut nodes, mut stops) = (0, 0, 0, 0);
    let mut all_nodes = std::collections::HashMap::new();
    let mut all_roads = Vec::new();
    let mut versions: BTreeMap<u32, usize> = BTreeMap::new();
    for (n, data) in &files {
        match Sector::parse_base(data) {
            Ok(s) => {
                ok += 1;
                *versions.entry(s.version).or_default() += 1;
                for (t, c) in &s.item_counts {
                    *types.entry(*t).or_default() += c;
                }
                roads += s.roads.len();
                prefabs += s.prefabs.len();
                nodes += s.nodes.len();
                stops += s.bus_stops.len();
                all_nodes.extend(s.nodes);
                all_roads.extend(s.roads);
            }
            Err(e) => {
                failed += 1;
                println!("{n}: {e:#}");
            }
        }
    }
    println!("sectors: {ok} read, {failed} failed; format versions {versions:?}");
    println!("roads {roads}, prefabs {prefabs}, bus stops {stops}, nodes {nodes}");
    println!("items by type: {types:?}");
    let linked = all_roads
        .iter()
        .filter(|r| all_nodes.contains_key(&r.start_node) && all_nodes.contains_key(&r.end_node))
        .count();
    println!(
        "roads with both nodes found: {linked} of {}",
        all_roads.len()
    );
    let mut looks: BTreeMap<String, usize> = BTreeMap::new();
    for r in &all_roads {
        *looks.entry(r.road_look.to_string()).or_default() += 1;
    }
    let mut looks: Vec<_> = looks.into_iter().collect();
    looks.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    println!("most used road looks: {:?}", &looks[..looks.len().min(12)]);
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for n in all_nodes.values() {
        for i in 0..3 {
            lo[i] = lo[i].min(n.pos[i]);
            hi[i] = hi[i].max(n.pos[i]);
        }
    }
    println!(
        "node extent: x {:.0}..{:.0}, y {:.0}..{:.0}, z {:.0}..{:.0}",
        lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]
    );
    Ok(())
}
