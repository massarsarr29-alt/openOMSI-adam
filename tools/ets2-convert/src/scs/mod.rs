//! Euro Truck Simulator 2 (Prism engine) map files, as documented by sk-zk's map-docs
//! (https://github.com/sk-zk/map-docs/wiki). Only what the conversion needs is kept; the
//! rest of each item is read past, so that the items after it are found.

pub mod reader;
pub mod scenery;
pub mod sector;
pub mod token;
