//! The files the writer produces read back with openOMSI's own loaders.

use ets2_convert::test_map;
use omsi_map::{ailists::AiLists, global::GlobalCfg, terrain::Terrain, tile::Tile};

/// A fresh copy of the test map for each test: the tests run in parallel.
fn dir(test: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ets2-convert-{}-{test}", std::process::id()));
    test_map::build().write(&d).unwrap();
    d
}

#[test]
fn global_cfg_reads_back() {
    let d = dir("global_cfg_reads_back");
    let g = GlobalCfg::load(&d.join("global.cfg")).unwrap();
    assert_eq!(g.name, "ets2-convert-test");
    assert_eq!(g.tiles.len(), 1);
    assert_eq!(
        (g.tiles[0].x, g.tiles[0].y, g.tiles[0].file.as_str()),
        (0, 0, "tile_0_0.map")
    );
    assert_eq!(g.entry_points.len(), 1);
    let e = &g.entry_points[0];
    assert_eq!((e.group, e.name.as_str()), (0, "Start"));
    assert_eq!(e.pos, [152.0, 20.0, 0.0]);
    assert!(g.unknown_keywords.is_empty(), "{:?}", g.unknown_keywords);
}

#[test]
fn tile_reads_back() {
    let d = dir("tile_reads_back");
    let t = Tile::load(&d.join("tile_0_0.map")).unwrap();
    assert!(t.has_terrain);
    assert_eq!(t.splines.len(), 2);
    let (a, b) = (&t.splines[0], &t.splines[1]);
    assert_eq!((a.id, a.prev_id, a.next_id), (1, 0, 2));
    assert_eq!((b.id, b.prev_id, b.next_id), (2, 1, 0));
    assert_eq!(a.pos, [150.0, 10.0, 0.0]);
    assert_eq!((a.length, a.radius), (140.0, 0.0));
    assert!((b.pos[1] - 150.0).abs() < 1e-9);
    assert_eq!(b.radius, 100.0);
    assert_eq!(b.tex_offset, 140.0);
    assert!(t.unknown_keywords.is_empty(), "{:?}", t.unknown_keywords);
    let terrain = Terrain::load(&d.join("tile_0_0.map.terrain")).unwrap();
    assert_eq!(terrain.samples(), 61);
}

#[test]
fn curve_ends_where_expected() {
    let m = test_map::build();
    let (end, heading) = m.tiles[0].splines[1].end();
    assert!(
        (end[0] - 250.0).abs() < 1e-9 && (end[1] - 250.0).abs() < 1e-9,
        "{end:?}"
    );
    assert!((heading - 90.0).abs() < 1e-9);
}

#[test]
fn ai_lists_read_back() {
    let d = dir("ai_lists_read_back");
    let ai = AiLists::load(&d.join("ailists.cfg")).unwrap();
    let g = format!("{ai:?}");
    assert!(
        g.contains("NormalCars") && g.contains("AI_VW_Golf_2.bus"),
        "{g}"
    );
}
