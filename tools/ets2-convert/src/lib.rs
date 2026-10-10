//! Converter from Euro Truck Simulator 2 maps to OMSI 2 maps: `scs` reads ETS 2's map
//! sectors, `convert` turns their roads into OMSI splines, and `writer` writes the OMSI
//! map folder (`test_map` builds a small map to check the writer with).

pub mod convert;
pub mod line;
pub mod objects;
pub mod scs;
pub mod test_map;
pub mod writer;
