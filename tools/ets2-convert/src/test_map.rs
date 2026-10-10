//! A one-tile map to check that a map written by this tool loads: a straight road that
//! runs into a right-hand curve, AI cars on it, and the bus starting on the road.

use crate::writer::{AiGroup, EntryPoint, Map, Spline, Tile};
use omsi_map::terrain::Terrain;

/// A two-lane street of the stock Berlin-Spandau content.
const ROAD: &str = "Splines\\Marcel\\str_2spur_11m_SeeburgerStr1.sli";

pub fn build() -> Map {
    let straight = Spline {
        file: ROAD.into(),
        id: 1,
        prev_id: 0,
        next_id: 2,
        pos: [150.0, 10.0, 0.0],
        heading: 0.0,
        length: 140.0,
        radius: 0.0,
        grad_start: 0.0,
        grad_end: 0.0,
        tex_offset: 0.0,
    };
    let (end, heading) = straight.end();
    let curve = Spline {
        id: 2,
        prev_id: 1,
        next_id: 0,
        pos: [end[0], end[1], 0.0],
        heading,
        // a quarter circle of 100 m to the right: it ends at (250, 250) heading east
        length: 100.0 * std::f64::consts::FRAC_PI_2,
        radius: 100.0,
        tex_offset: straight.length,
        ..straight.clone()
    };
    Map {
        name: "ets2-convert-test".into(),
        friendly_name: "ETS 2 convert - test".into(),
        description: "Test map written by ets2-convert: one road, one curve.".into(),
        ground_texture: ("texture\\gras.bmp".into(), "texture\\gras_det.bmp".into()),
        tiles: vec![Tile {
            x: 0,
            y: 0,
            splines: vec![straight, curve],
            terrain: Some(Terrain::flat()),
            ..Default::default()
        }],
        entry_points: vec![EntryPoint {
            name: "Start".into(),
            tile: 0,
            // the right-hand lane at the start of the road
            pos: [152.0, 20.0, 0.0],
            heading: 0.0,
        }],
        ai_groups: vec![AiGroup {
            name: "NormalCars".into(),
            vehicles: vec![
                ("vehicles\\VW_Golf_2\\AI_VW_Golf_2.bus".into(), 70),
                ("vehicles\\MB_W123_230E\\AI_mb_w123_230e.bus".into(), 50),
                ("vehicles\\Citr_BX\\BX.ovh".into(), 40),
            ],
        }],
        ..Default::default()
    }
}
