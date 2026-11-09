//! The enumerations of `py_premiere/enums/general.py` (Premiere ExtendScript values).
//!
//! Each enum has `value()` (the Python `IntEnum` value), `name()` (the member name, e.g.
//! `"BEZIER"`) and `from_value(i64) -> Result<Self>`, which refuses an unknown value with
//! Python's own message (`"99 is not a valid KeyframeInterpolation"`).

use crate::error::{Error, Result};

macro_rules! py_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident = $value:expr => $pyname:literal,)+ }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            /// Every member, in definition order.
            pub const ALL: &'static [$name] = &[$($name::$variant,)+];

            /// The `IntEnum` value.
            pub fn value(self) -> i64 {
                match self {
                    $($name::$variant => $value,)+
                }
            }

            /// The member name (`.name`).
            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $pyname,)+
                }
            }

            /// `Enum(value)`: the member with this value.
            pub fn from_value(value: i64) -> Result<$name> {
                match value {
                    $(v if v == $value => Ok($name::$variant),)+
                    _ => Err(Error::Value(format!("{} is not a valid {}", value, stringify!($name)))),
                }
            }

            /// `Enum[name]`: the member with this name.
            pub fn from_name(name: &str) -> Result<$name> {
                match name {
                    $($pyname => Ok($name::$variant),)+
                    _ => Err(Error::Key(crate::convert::model::py_str_repr(name))),
                }
            }
        }
    };
}

py_enum! {
    /// `ProjectItemType`: `CLIP`, `BIN`, `ROOT` or `FILE` (py_premiere never parses `FILE`).
    ProjectItemType {
        Clip = 1 => "CLIP",
        Bin = 2 => "BIN",
        Root = 3 => "ROOT",
        File = 4 => "FILE",
    }
}

py_enum! {
    /// `KeyframeInterpolation`: temporal interpolation of a keyframe.
    KeyframeInterpolation {
        Linear = 0 => "LINEAR",
        EaseInObsolete = 1 => "EASE_IN_OBSOLETE",
        EaseOutObsolete = 2 => "EASE_OUT_OBSOLETE",
        EaseInEaseOutObsolete = 3 => "EASE_IN_EASE_OUT_OBSOLETE",
        Hold = 4 => "HOLD",
        Bezier = 5 => "BEZIER",
        Time = 6 => "TIME",
        TimeTransitionStart = 7 => "TIME_TRANSITION_START",
        TimeTransitionEnd = 8 => "TIME_TRANSITION_END",
    }
}

/// `CaptionFormat` lives with the caption port.
pub use crate::caption::CaptionFormat;

py_enum! {
    /// `BlendMode`: the GUI-domain values (the Opacity intrinsic's `ParameterID` 3 param).
    BlendMode {
        Normal = 0 => "NORMAL",
        Dissolve = 1 => "DISSOLVE",
        Darken = 3 => "DARKEN",
        Multiply = 4 => "MULTIPLY",
        ColorBurn = 5 => "COLOR_BURN",
        LinearBurn = 6 => "LINEAR_BURN",
        DarkerColor = 7 => "DARKER_COLOR",
        Lighten = 9 => "LIGHTEN",
        Screen = 10 => "SCREEN",
        ColorDodge = 11 => "COLOR_DODGE",
        LinearDodge = 12 => "LINEAR_DODGE",
        LighterColor = 13 => "LIGHTER_COLOR",
        Overlay = 15 => "OVERLAY",
        SoftLight = 16 => "SOFT_LIGHT",
        HardLight = 17 => "HARD_LIGHT",
        VividLight = 18 => "VIVID_LIGHT",
        LinearLight = 19 => "LINEAR_LIGHT",
        PinLight = 20 => "PIN_LIGHT",
        HardMix = 21 => "HARD_MIX",
        Difference = 23 => "DIFFERENCE",
        Exclusion = 24 => "EXCLUSION",
        Subtract = 25 => "SUBTRACT",
        Divide = 26 => "DIVIDE",
        Hue = 28 => "HUE",
        Saturation = 29 => "SATURATION",
        Color = 30 => "COLOR",
        Luminosity = 31 => "LUMINOSITY",
    }
}

py_enum! {
    /// `AlphaUsage`: how a still's alpha channel is interpreted.
    AlphaUsage {
        None = 0 => "NONE",
        Straight = 1 => "STRAIGHT",
        Premultiplied = 2 => "PREMULTIPLIED",
        Ignore = 3 => "IGNORE",
    }
}

py_enum! {
    /// `VideoFieldType`: field dominance of interlaced footage.
    VideoFieldType {
        Default = -1 => "DEFAULT",
        Progressive = 0 => "PROGRESSIVE",
        UpperFirst = 1 => "UPPER_FIRST",
        LowerFirst = 2 => "LOWER_FIRST",
    }
}

py_enum! {
    /// `GeneratorType`: the big-endian fourcc Premiere stores as a generator's `FilePath`.
    GeneratorType {
        BlackVideo = 1112293707 => "BLACK_VIDEO",
        ColorMatte = 1129270354 => "COLOR_MATTE",
        BarsAndTone = 1111577171 => "BARS_AND_TONE",
        TransparentVideo = 1414680150 => "TRANSPARENT_VIDEO",
        UniversalCountingLeader = 1279607108 => "UNIVERSAL_COUNTING_LEADER",
    }
}

py_enum! {
    /// `TimeInterpolationType`: how a retimed clip fills in the frames it needs.
    TimeInterpolationType {
        FrameSampling = 0 => "FRAME_SAMPLING",
        FrameBlending = 1 => "FRAME_BLENDING",
        OpticalFlow = 2 => "OPTICAL_FLOW",
    }
}
