"""General enumerations from the Premiere Scripting Guide."""

from __future__ import annotations

from enum import IntEnum


class ProjectItemType(IntEnum):
    """The type of a `ProjectItem`.

    One of `CLIP`, `BIN`, `ROOT` or `FILE` [Premiere Scripting Guide
    `ProjectItem.type`].
    """

    CLIP = 1
    BIN = 2
    ROOT = 3
    FILE = 4


class KeyframeInterpolation(IntEnum):
    """Temporal interpolation of a keyframe.

    Values match the Scripting Guide's `kfInterpMode` constants and UXP's
    `Constants.InterpolationMode`; verified against the stored keyframe
    encoding via the `09_keyframes` fixture.
    """

    LINEAR = 0
    EASE_IN_OBSOLETE = 1
    EASE_OUT_OBSOLETE = 2
    EASE_IN_EASE_OUT_OBSOLETE = 3
    HOLD = 4
    BEZIER = 5
    TIME = 6
    TIME_TRANSITION_START = 7
    TIME_TRANSITION_END = 8


class CaptionFormat(IntEnum):
    """The broadcast format of a caption track.

    Values are ExtendScript's `Sequence.CAPTION_FORMAT_*` constants, read
    straight off the running application (`sweep_caption_format.jsx`).
    The three Teletext variants pack a sub-format in the high word, which
    is exactly how the file stores them: `Format` = the low word,
    `SubFormat` = the high word, and `SUBTITLE` (0) elides both.
    """

    SUBTITLE = 0
    CEA_608 = 1
    CEA_708 = 2
    TELETEXT = 3
    EBU_SUBTITLE = 0x10003
    OP42 = 0x20003
    OP47 = 0x30003


class BlendMode(IntEnum):
    """A clip's compositing blend mode.

    Values are what Premiere's GUI stores (the Opacity intrinsic's
    `ParameterID` 3 param, bounds 0..31): the blend-mode popup's rows in
    order, with the five separators holding the missing values 2, 8, 14,
    22 and 27 (81_blend_modes). Neither ExtendScript nor UXP names these;
    the API-facing twin (`ParameterID` 2, bounds 0..27) uses its own
    near-alphabetical numbering, mapped in `models/track_item.py` - the
    GUI writes both, and so does py.
    """

    NORMAL = 0
    DISSOLVE = 1
    DARKEN = 3
    MULTIPLY = 4
    COLOR_BURN = 5
    LINEAR_BURN = 6
    DARKER_COLOR = 7
    LIGHTEN = 9
    SCREEN = 10
    COLOR_DODGE = 11
    LINEAR_DODGE = 12
    LIGHTER_COLOR = 13
    OVERLAY = 15
    SOFT_LIGHT = 16
    HARD_LIGHT = 17
    VIVID_LIGHT = 18
    LINEAR_LIGHT = 19
    PIN_LIGHT = 20
    HARD_MIX = 21
    DIFFERENCE = 23
    EXCLUSION = 24
    SUBTRACT = 25
    DIVIDE = 26
    HUE = 28
    SATURATION = 29
    COLOR = 30
    LUMINOSITY = 31


class AlphaUsage(IntEnum):
    """How a still's alpha channel is interpreted.

    Values from the UXP `FootageInterpretation.ALPHACHANNEL_*` constants.
    """

    NONE = 0
    STRAIGHT = 1
    PREMULTIPLIED = 2
    IGNORE = 3


class VideoFieldType(IntEnum):
    """Field dominance of interlaced footage.

    Values from the UXP `FootageInterpretation.FIELD_TYPE_*` constants.
    """

    DEFAULT = -1
    PROGRESSIVE = 0
    UPPER_FIRST = 1
    LOWER_FIRST = 2


class GeneratorType(IntEnum):
    """Which of Premiere's synthetic media an item is backed by.

    Generated media has no file: its `Media/FilePath` is a big-endian
    four-character code instead of a path, which is why ExtendScript reports
    no media path for these items. The codes are the values here (`BLAK`,
    `COLR`, ...), captured by creating one of each through the QE DOM.
    """

    BLACK_VIDEO = 1112293707
    COLOR_MATTE = 1129270354
    BARS_AND_TONE = 1111577171
    TRANSPARENT_VIDEO = 1414680150
    UNIVERSAL_COUNTING_LEADER = 1279607108


class TimeInterpolationType(IntEnum):
    """How a retimed clip fills in the frames it needs.

    Premiere elides the field at `FRAME_SAMPLING`, so an untouched clip
    reports that. The mapping was pinned by writing each value through the QE
    DOM: legacy `setFrameBlend(true)` and `setTimeInterpolationType(1)`
    produce the same stored `1`.
    """

    FRAME_SAMPLING = 0
    FRAME_BLENDING = 1
    OPTICAL_FLOW = 2
