"""The `Time` value object."""

from __future__ import annotations

import math

from .validators import validate_int, validate_number, validate_string

#: Premiere's tick rate [Premiere Scripting Guide `Time` object].
TICKS_PER_SECOND = 254016000000

#: ExtendScript's sentinel for an unset time (-400000 s): what the DOM
#: reports where the file stores nothing. py maps it to `None`.
UNSET_TICKS = -400000 * TICKS_PER_SECOND

_validate_timecode_string = validate_string(allow_empty=False)


def _timebase_ticks(timebase: object) -> int:
    """The tick count of a timebase given as a `Time` or a raw int.

    A timebase is ticks per frame, the unit `FrameRate` elements store
    (8475667200 for 29.97 fps).
    """
    if isinstance(timebase, Time):
        ticks = timebase.ticks
    else:
        validate_int(timebase)
        ticks = timebase  # type: ignore[assignment]
    if ticks <= 0:
        raise ValueError("timebase must be a positive number of ticks per frame")
    return ticks


def _nominal_fps(timebase_ticks: int) -> int:
    # 29.97 rounds to the 30 its timecode counts in, 23.976 to 24, etc.
    return round(TICKS_PER_SECOND / timebase_ticks)


def _drop_count(timebase_ticks: int) -> int:
    # Drop-frame timecode drops 2 frame NUMBERS per minute at 29.97 and 4
    # at 59.94 (never actual frames); it only exists for the 30-fps family.
    nominal = _nominal_fps(timebase_ticks)
    if nominal % 30 or nominal * timebase_ticks == TICKS_PER_SECOND:
        raise ValueError("drop-frame timecode requires a 29.97/59.94 family timebase")
    return nominal // 15


class Time:
    """An object representing a time.

    Internally, the time is computed in `ticks`; there are 254016000000
    ticks per second [Premiere Scripting Guide `Time` object].
    """

    def __init__(self, ticks: int = 0) -> None:
        self.ticks = ticks

    @property
    def ticks(self) -> int:
        """The time value, expressed in ticks. Read/write."""
        return self._ticks

    @ticks.setter
    def ticks(self, value: int) -> None:
        validate_int(value)
        self._ticks = value

    @property
    def seconds(self) -> float:
        """The time value, expressed in seconds. Read/write."""
        return self._ticks / TICKS_PER_SECOND

    @seconds.setter
    def seconds(self, value: float) -> None:
        validate_number(value)
        ticks = value * TICKS_PER_SECOND
        if not math.isfinite(ticks):
            # A finite number of seconds can still overflow to infinity once
            # scaled by the tick rate; `round` would raise OverflowError.
            raise ValueError(f"{value} seconds is too large to express in ticks")
        self._ticks = round(ticks)

    @classmethod
    def from_seconds(cls, seconds: float) -> Time:
        """A `Time` from a number of seconds, rounded to the nearest tick."""
        time = cls()
        time.seconds = seconds
        return time

    @classmethod
    def from_frames(cls, frames: int, timebase: Time | int) -> Time:
        """A `Time` from a frame count.

        `timebase` is ticks per frame (the unit `FrameRate` elements and
        `Sequence.timebase` use), given as an int or a `Time`.
        """
        validate_int(frames)
        return cls(frames * _timebase_ticks(timebase))

    def to_frames(self, timebase: Time | int) -> int:
        """The frame this time falls in (floored to a whole frame)."""
        return self._ticks // _timebase_ticks(timebase)

    @classmethod
    def from_timecode(cls, timecode: str, timebase: Time | int) -> Time:
        """A `Time` from a `HH:MM:SS:FF` timecode string.

        A `;` before the frame field (`01:00:00;02`) marks drop-frame,
        which only exists for the 29.97/59.94 family.
        """
        _validate_timecode_string(timecode)
        drop = ";" in timecode
        parts = timecode.replace(";", ":").split(":")
        if len(parts) != 4 or not all(part.isdigit() for part in parts):
            raise ValueError(f"not a HH:MM:SS:FF timecode: {timecode!r}")
        hours, minutes, seconds, frames = (int(part) for part in parts)
        ticks = _timebase_ticks(timebase)
        nominal = _nominal_fps(ticks)
        if minutes >= 60 or seconds >= 60 or frames >= nominal:
            raise ValueError(f"timecode field out of range: {timecode!r}")
        number = ((hours * 60 + minutes) * 60 + seconds) * nominal + frames
        if drop:
            dropped = _drop_count(ticks)
            total_minutes = hours * 60 + minutes
            if seconds == 0 and minutes % 10 and frames < dropped:
                # The first frame numbers of a dropped minute do not exist.
                raise ValueError(f"dropped frame number: {timecode!r}")
            number -= dropped * (total_minutes - total_minutes // 10)
        return cls(number * ticks)

    def to_timecode(self, timebase: Time | int, drop_frame: bool = False) -> str:
        """This time as a `HH:MM:SS:FF` string (floored to a whole frame).

        With `drop_frame`, counts in drop-frame and separates the frame
        field with `;`, as Premiere displays 29.97/59.94 material.
        """
        ticks = _timebase_ticks(timebase)
        if self._ticks < 0:
            raise ValueError("cannot format a negative time as timecode")
        number = self._ticks // ticks
        nominal = _nominal_fps(ticks)
        if drop_frame:
            dropped = _drop_count(ticks)
            per_ten_minutes = nominal * 600 - dropped * 9
            per_minute = nominal * 60 - dropped
            tens, rest = divmod(number, per_ten_minutes)
            if rest > dropped:
                number += dropped * 9 * tens + dropped * (
                    (rest - dropped) // per_minute
                )
            else:
                number += dropped * 9 * tens
        seconds_total, frames = divmod(number, nominal)
        minutes_total, seconds = divmod(seconds_total, 60)
        hours, minutes = divmod(minutes_total, 60)
        separator = ";" if drop_frame else ":"
        return f"{hours:02d}:{minutes:02d}:{seconds:02d}{separator}{frames:02d}"

    def __eq__(self, other: object) -> bool:
        # No paired __hash__: `ticks` is writable, and mutating a Time held
        # in a set or used as a dict key would corrupt the container, so
        # defining `__eq__` alone deliberately leaves the class unhashable.
        if not isinstance(other, Time):
            return NotImplemented
        return self._ticks == other._ticks

    def __repr__(self) -> str:
        return f"Time(ticks={self._ticks})"

    def __add__(self, other: object) -> Time:
        if not isinstance(other, Time):
            return NotImplemented
        return Time(self._ticks + other._ticks)

    def __sub__(self, other: object) -> Time:
        if not isinstance(other, Time):
            return NotImplemented
        return Time(self._ticks - other._ticks)

    def __mul__(self, other: object) -> Time:
        if not isinstance(other, (int, float)):
            return NotImplemented
        return Time(round(self._ticks * other))

    __rmul__ = __mul__

    def __floordiv__(self, other: object) -> Time:
        # int only: `int // float` yields a float the ticks setter rejects;
        # timedelta refuses float floor-division for the same reason.
        if not isinstance(other, int):
            return NotImplemented
        return Time(self._ticks // other)

    def __truediv__(self, other: object) -> Time:
        if not isinstance(other, (int, float)):
            return NotImplemented
        return Time(round(self._ticks / other))

    def __neg__(self) -> Time:
        return Time(-self._ticks)

    def __abs__(self) -> Time:
        return Time(abs(self._ticks))

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Time):
            return NotImplemented
        return self._ticks < other._ticks

    def __le__(self, other: object) -> bool:
        if not isinstance(other, Time):
            return NotImplemented
        return self._ticks <= other._ticks

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, Time):
            return NotImplemented
        return self._ticks > other._ticks

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, Time):
            return NotImplemented
        return self._ticks >= other._ticks


def validate_time(value: object, instance: object | None = None) -> None:
    """Validate that a value is a `Time`.

    Lives here rather than in `validators.py` because that module is imported
    by this one, so it cannot name the class. Takes the same `(value,
    instance)` shape as the validators, so it can also back an `XmlField`.
    """
    if not isinstance(value, Time):
        raise TypeError(f"expected a Time, got {type(value).__name__}")
