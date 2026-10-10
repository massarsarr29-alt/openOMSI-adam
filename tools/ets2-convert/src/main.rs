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
    /// Converts the roads around an ETS 2 city, or in a rectangle, into an OMSI map folder.
    Convert {
        archive: PathBuf,
        out: PathBuf,
        /// City name as the map defines it, e.g. "vincennes".
        #[arg(long, required_unless_present = "rect")]
        city: Option<String>,
        /// A rectangle in ETS 2 coordinates instead of a city: "min_x,min_z,max_x,max_z".
        #[arg(long, conflicts_with = "city")]
        rect: Option<String>,
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
        /// A bus line to add: a file with the line number, its minutes and its stops (see
        /// `line::LineSpec::parse`).
        #[arg(long)]
        line: Option<PathBuf>,
        /// Game files to take the models from, first one first: the map mod, folders or zips
        /// of extracted files, the game's own archives (def.scs, base.scs...). With at least
        /// one, the map's model items (signs, trees, buildings) become scenery objects.
        #[arg(long = "source")]
        sources: Vec<PathBuf>,
        /// Folder under `Sceneryobjects` the objects are written to.
        #[arg(long, default_value = "ets2-idf")]
        objects: String,
        /// The OMSI 2 folder (defaults to the one the out folder is in, `<OMSI 2>/maps/x`).
        #[arg(long)]
        omsi: Option<PathBuf>,
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
            rect,
            tiles,
            spline,
            name,
            line,
            sources,
            objects,
            omsi,
        } => {
            use ets2_convert::convert::{convert, Area, Options, World};
            let mut world = World::default();
            for (n, data) in sector_files(&archive, "base")? {
                let s = ets2_convert::scs::sector::Sector::parse_base(&data)
                    .with_context(|| format!("reading {n}"))?;
                world.add(s);
            }
            if !sources.is_empty() {
                for (n, data) in sector_files(&archive, "aux")? {
                    let s = ets2_convert::scs::sector::Sector::parse_aux(&data)
                        .with_context(|| format!("reading {n}"))?;
                    world.add(s);
                }
            }
            let area = match (city, rect) {
                (Some(city), _) => Area::around(
                    world
                        .city_centre(&city)
                        .with_context(|| format!("no city called {city} in the map"))?,
                    tiles,
                ),
                (None, Some(rect)) => {
                    let v: Vec<f64> = rect
                        .split(',')
                        .map(|v| v.trim().parse())
                        .collect::<Result<_, _>>()
                        .context("--rect wants four numbers")?;
                    anyhow::ensure!(v.len() == 4, "--rect wants four numbers");
                    Area {
                        min: [v[0], v[1]],
                        max: [v[2], v[3]],
                    }
                }
                (None, None) => unreachable!("clap requires one"),
            };
            let folder = out
                .file_name()
                .map_or("ets2".into(), |n| n.to_string_lossy().into_owned());
            let name = name.unwrap_or_else(|| folder.clone());
            let (mut map, net) = convert(
                &world,
                &Options {
                    name,
                    area,
                    road_spline: spline,
                    // the helper spline lives in the map folder, under the OMSI root
                    helper_spline: format!("maps\\{folder}\\ets2_helper.sli"),
                },
            );
            map.extra_files.push((
                "ets2_helper.sli".into(),
                ets2_convert::writer::HELPER_SPLINE.into(),
            ));
            if let Some(line) = line {
                let text = std::fs::read_to_string(&line)
                    .with_context(|| format!("reading {}", line.display()))?;
                let spec = ets2_convert::line::LineSpec::parse(&text)?;
                for l in ets2_convert::line::build(&mut map, &net, &area, &spec) {
                    println!("{l}");
                }
            }
            if !sources.is_empty() {
                let mut vfs = ets2_convert::scs::vfs::Vfs::default();
                vfs.add(&archive)?;
                for s in &sources {
                    vfs.add(s)?;
                }
                let defs = ets2_convert::objects::model_defs(&vfs)?;
                println!("{} model definitions", defs.len());
                let mut made = ets2_convert::objects::Objects::new(&vfs, &objects);
                for l in ets2_convert::objects::place(&mut map, &area, &world, &defs, &mut made) {
                    println!("{l}");
                }
                let prefabs = ets2_convert::objects::prefab_defs(&vfs)?;
                for l in ets2_convert::objects::place_prefabs(&mut map, &area, &world, &prefabs, &mut made) {
                    println!("{l}");
                }
                let root = match omsi {
                    Some(r) => r,
                    None => out
                        .parent()
                        .and_then(|p| p.parent())
                        .context("give --omsi: the out folder is not in <OMSI 2>/maps")?
                        .to_path_buf(),
                };
                let dir = root.join("Sceneryobjects").join(&objects);
                for (name, bytes) in &made.files {
                    let p = dir.join(name.replace('\\', "/"));
                    std::fs::create_dir_all(p.parent().unwrap())?;
                    std::fs::write(&p, bytes).with_context(|| format!("writing {}", p.display()))?;
                }
                println!("{} object files written to {}", made.files.len(), dir.display());
            }
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

/// The sector files (`.base` or `.aux`) of a map mod archive (or a zip of its map folder),
/// or of a folder its "map" folder was copied to.
fn sector_files(archive: &std::path::Path, ext: &str) -> Result<Vec<(String, Vec<u8>)>> {
    use std::io::Read;
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    if archive.is_dir() {
        let mut stack = vec![archive.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)? {
                let p = e?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == ext) {
                    files.push((p.display().to_string(), std::fs::read(&p)?));
                }
            }
        }
    } else {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(archive)?)?;
        let names: Vec<String> = zip
            .file_names()
            .filter(|n| n.ends_with(&format!(".{ext}")))
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
    let files = sector_files(archive, "base")?;
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
