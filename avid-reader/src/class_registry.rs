//! Static FourCC-to-class dispatch from py-avb's runtime registration table.
//!
//! Source: pinned py-avb `ff64e06b60077ecabe5db3fef239898b08363898`
//! (`avb.utils.AVBClaseID_dict`, populated by registered classes at import).
//! The four base declarations are intentionally not runtime file classes.
//! This is typed lookup, not Python's dynamic registration interface.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredClass {
    pub fourcc: [u8; 4],
    pub class_name: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassLookupError {
    UnregisteredBase([u8; 4]),
    Unknown([u8; 4]),
}

impl std::fmt::Display for ClassLookupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnregisteredBase(id) => write!(
                formatter,
                "{} is an unregistered base declaration",
                String::from_utf8_lossy(id)
            ),
            Self::Unknown(id) => write!(
                formatter,
                "{} has no registered py-avb class",
                String::from_utf8_lossy(id)
            ),
        }
    }
}

impl std::error::Error for ClassLookupError {}

pub const UNREGISTERED_BASES: [[u8; 4]; 4] = [*b"ASET", *b"CLIP", *b"COMP", *b"WARP"];

/// Name-only helper registrations in pinned py-avb's `AVBClassName_dict`.
/// They have no independently constructible file FourCC; typed helper data
/// remains the responsibility of its owning object and factory APIs.
pub const HELPER_CLASS_NAMES: [&str; 12] = [
    "ASPIPlugin",
    "ASPIPluginChunk",
    "AVBRefList",
    "BinItem",
    "ControlPoint",
    "ControlPointProperty",
    "EffectParam",
    "EqualizerBand",
    "ParamControlPoint",
    "ParamControlPointProperty",
    "SiftItem",
    "Track",
];

pub fn is_registered_helper_class_name(name: &str) -> bool {
    HELPER_CLASS_NAMES.binary_search(&name).is_ok()
}

macro_rules! registered {
    ($id:expr, $name:literal) => {
        RegisteredClass {
            fourcc: *$id,
            class_name: $name,
        }
    };
}

/// Sorted by FourCC so lookup can use binary search without allocations.
pub const REGISTERED_CLASSES: [RegisteredClass; 66] = [
    registered!(b"ABIN", "Bin"),
    registered!(b"ABOB", "BOBPosition"),
    registered!(b"AIFC", "AIFCDescriptor"),
    registered!(b"ANCD", "ANCDataDescriptor"),
    registered!(b"APOS", "Position"),
    registered!(b"ASPI", "AudioSuitePluginEffect"),
    registered!(b"ATTR", "Attributes"),
    registered!(b"AVUP", "CFUserParam"),
    registered!(b"BINF", "BinFirst"),
    registered!(b"BVst", "BinViewSetting"),
    registered!(b"CCFX", "ColorCorrectionEffect"),
    registered!(b"CDCI", "CDCIDescriptor"),
    registered!(b"CMPO", "Composition"),
    registered!(b"CTRL", "ControlClip"),
    registered!(b"DATD", "DataDescriptor"),
    registered!(b"DIDD", "DIDDescriptor"),
    registered!(b"DIDP", "DIDPosition"),
    registered!(b"ECCP", "Edgecode"),
    registered!(b"EQMB", "EqualizerMultiBand"),
    registered!(b"FILE", "MacFileLocator"),
    registered!(b"FILL", "Filler"),
    registered!(b"FXPS", "EffectParamList"),
    registered!(b"GRFX", "GraphicEffect"),
    registered!(b"JPED", "JPEGDescriptor"),
    registered!(b"MASK", "CaptureMask"),
    registered!(b"MCBR", "BinRef"),
    registered!(b"MCMR", "MobRef"),
    registered!(b"MDES", "MediaDescriptor"),
    registered!(b"MDFL", "MediaFileDescriptor"),
    registered!(b"MDFM", "FilmDescriptor"),
    registered!(b"MDNG", "NagraDescriptor"),
    registered!(b"MDTP", "TapeDescriptor"),
    registered!(b"MPGA", "MPGADescriptor"),
    registered!(b"MPGI", "MPGIDescriptor"),
    registered!(b"MPGP", "MPGPosition"),
    registered!(b"MSML", "MSMLocator"),
    registered!(b"MULD", "MultiDescriptor"),
    registered!(b"PCMA", "PCMADescriptor"),
    registered!(b"PRCL", "ParamClip"),
    registered!(b"PRIT", "ParameterItem"),
    registered!(b"PRLS", "ParameterList"),
    registered!(b"PVOL", "PanVolumeEffect"),
    registered!(b"REPT", "Repeat"),
    registered!(b"RGBA", "RGBADescriptor"),
    registered!(b"RSET", "EssenceGroup"),
    registered!(b"SCLP", "SourceClip"),
    registered!(b"SEQU", "Sequence"),
    registered!(b"SHLP", "ShapeList"),
    registered!(b"SLCT", "Selector"),
    registered!(b"SPED", "MotionEffect"),
    registered!(b"STRB", "StrobeEffect"),
    registered!(b"TCCP", "Timecode"),
    registered!(b"TKDA", "TrackerData"),
    registered!(b"TKDS", "TrackerDataSlot"),
    registered!(b"TKFX", "TrackEffect"),
    registered!(b"TKMN", "TrackerManager"),
    registered!(b"TKPA", "TrackerParameter"),
    registered!(b"TKPS", "TrackerParameterSlot"),
    registered!(b"TMBC", "Marker"),
    registered!(b"TMCS", "TimeCrumbList"),
    registered!(b"TNFX", "TransitionEffect"),
    registered!(b"TRKG", "TrackGroup"),
    registered!(b"TRKR", "TrackRef"),
    registered!(b"URLL", "URLLocator"),
    registered!(b"WAVE", "WaveDescriptor"),
    registered!(b"WINF", "WinFileLocator"),
];

pub fn registered_class(fourcc: [u8; 4]) -> Result<&'static RegisteredClass, ClassLookupError> {
    match REGISTERED_CLASSES.binary_search_by_key(&fourcc, |entry| entry.fourcc) {
        Ok(index) => Ok(&REGISTERED_CLASSES[index]),
        Err(_) if UNREGISTERED_BASES.contains(&fourcc) => {
            Err(ClassLookupError::UnregisteredBase(fourcc))
        }
        Err(_) => Err(ClassLookupError::Unknown(fourcc)),
    }
}

pub fn registered_class_name(fourcc: [u8; 4]) -> Result<&'static str, ClassLookupError> {
    Ok(registered_class(fourcc)?.class_name)
}
