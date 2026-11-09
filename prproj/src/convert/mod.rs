//! The converter's Premiere logic (Rust port Phase 4): the JSON edit model (`avid2prproj.model/1`)
//! in, a Premiere project out. Ports `converter/src/avid2prproj/*.py`; no Avid knowledge here —
//! the Avid reader produces the model. One module per Python file:
//!
//! | Python | Rust |
//! |---|---|
//! | `model.py` | [`model`] |
//! | `opatom.py` | [`opatom`] |
//! | `metadata.py` | [`metadata`] |
//! | `audio.py` | [`audio`] |
//! | `combined.py` | [`combined`] |
//! | `placement.py` | [`placement`] |
//! | `markers.py` | [`markers`] |
//! | `colors.py` | [`colors`] |
//! | `lumetri.py` | [`lumetri`] |
//! | `motion.py` | [`motion`] |
//! | `avid_color.py` | [`avid_color`] |
//! | `writer.py` | [`writer`] |

pub mod audio;
pub mod avid_color;
pub mod colors;
pub mod combined;
pub mod lumetri;
pub mod markers;
pub mod matte;
pub mod metadata;
pub mod motion;
pub mod model;
pub mod opatom;
pub mod native_av;
pub mod placement;
pub mod printable;
pub mod rewrap;
pub mod time_remap;
pub mod writer;
pub mod prepared;
pub mod multicam;
pub mod original;
