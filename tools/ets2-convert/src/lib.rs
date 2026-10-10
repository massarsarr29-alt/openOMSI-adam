//! Converter from Euro Truck Simulator 2 maps to OMSI 2 maps. For now it writes OMSI maps
//! only (`writer`) and builds a small test map with them (`test_map`); reading ETS 2's
//! files comes next.

pub mod test_map;
pub mod writer;
