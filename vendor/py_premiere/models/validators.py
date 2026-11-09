"""Reusable validator factories for model fields (ported from py-aep).

Each factory returns a callable `(value, instance) -> None` that raises
`ValueError` or `TypeError` when the value is invalid. Pass the returned
callable as the `validate` parameter of a descriptor, or call it at the top
of a `@property` setter or public `__init__`.

The `instance` argument is the model object being modified, allowing
cross-field validation; module-level singletons cover the common cases.
"""

from __future__ import annotations

import math
import os
import re
from pathlib import Path
from typing import TYPE_CHECKING

from .color import Color

if TYPE_CHECKING:
    from typing import Callable, Iterable


def _validate_number(
    *,
    min: float | Callable[..., float | None] | None = None,
    max: float | Callable[..., float | None] | None = None,
    integer: bool = False,
    exclusive_min: bool = False,
) -> Callable[..., None]:
    """Return a validator that checks a numeric value.

    Args:
        min: Minimum allowed value. May be a static number or a callable
            `(instance) -> float` for dynamic bounds. Inclusive unless
            `exclusive_min` is set.
        max: Maximum allowed value (inclusive). May be a static number
            or a callable `(instance) -> float` for dynamic bounds.
        integer: When `True`, reject non-`int` values.
        exclusive_min: When `True`, the value must be strictly greater than
            `min`.
    """
    type_label = "an integer" if integer else "a number"

    def _validator(value: object, instance: object | None = None) -> None:
        if integer and (not isinstance(value, int) or isinstance(value, bool)):
            # `bool` subclasses `int`; an isinstance check alone would let
            # `True` through as 1. Not `type(value) is not int`: that would
            # also reject IntEnum members.
            raise TypeError(f"expected {type_label}, got {type(value).__name__}")
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            raise TypeError(f"expected {type_label}, got {type(value).__name__}")
        if isinstance(value, float) and not math.isfinite(value):
            # NaN/inf pass any bound comparison (always False for NaN) and
            # would serialize as a corrupt value.
            raise ValueError(f"must be a finite number, got {value}")
        lo = min(instance) if callable(min) else min
        hi = max(instance) if callable(max) else max
        if lo is not None:
            if exclusive_min and value <= lo:
                raise ValueError(f"must be > {lo}, got {value}")
            if not exclusive_min and value < lo:
                raise ValueError(f"must be >= {lo}, got {value}")
        if hi is not None and value > hi:
            raise ValueError(f"must be <= {hi}, got {value}")

    return _validator


#: One shared validator per enum class, so default-validator callers hold
#: identity-equal instances.
_ENUM_VALIDATORS: dict[type, Callable[..., None]] = {}


def validate_enum(enum_cls: type) -> Callable[..., None]:
    """Return a validator that checks value is a member of `enum_cls`.

    Accepts both enum instances and their int equivalents. An int that is
    not a valid member value raises `ValueError`; any other wrong type
    raises `TypeError`. The returned validator is memoized per `enum_cls`.
    """
    cached = _ENUM_VALIDATORS.get(enum_cls)
    if cached is not None:
        return cached

    def _validator(value: object, instance: object | None = None) -> None:
        if isinstance(value, enum_cls):
            return
        # Before the int branch: `bool` subclasses `int` and would silently
        # select the member whose value is 1.
        if isinstance(value, bool):
            raise TypeError(
                f"expected a {enum_cls.__name__}, got {type(value).__name__}"
            )
        if isinstance(value, int):
            try:
                enum_cls(value)
            except ValueError:
                raise ValueError(
                    f"{value!r} is not a valid {enum_cls.__name__} value"
                ) from None
            return
        raise TypeError(f"expected a {enum_cls.__name__}, got {type(value).__name__}")

    _ENUM_VALIDATORS[enum_cls] = _validator
    return _validator


def validate_one_of(allowed: Iterable[object]) -> Callable[..., None]:
    """Return a validator that checks value is in the allowed set."""
    allowed_list = list(allowed)
    allowed_set = set(allowed_list)
    formatted = ", ".join(str(v) for v in allowed_list)

    def _validator(value: object, instance: object | None = None) -> None:
        if value not in allowed_set:
            raise ValueError(f"must be one of [{formatted}], got {value!r}")

    return _validator


#: Characters XML 1.0 has no representation for; tab, LF and CR are the only
#: control characters a document may carry. Writing one produces a file
#: py cannot re-parse, and a NUL hangs Premiere on open.
_XML_ILLEGAL = re.compile(r"[\x00-\x08\x0b\x0c\x0e-\x1f]")

#: A value ending in a newline plus tabs is indistinguishable from the
#: serializer's block-form indentation, so py and Premiere read the saved
#: file differently (py keeps the newline, Premiere drops it).
_AMBIGUOUS_TAIL = re.compile(r"\n\t+\Z")


def validate_string(
    *,
    allow_empty: bool = True,
    max_length: int | None = None,
) -> Callable[..., None]:
    """Return a validator that checks a string value.

    Rejects text that cannot survive a save: characters XML 1.0 forbids,
    unpaired surrogates (which have no UTF-8 encoding), and a trailing
    newline-plus-tabs run the serializer would emit as indentation.

    Args:
        allow_empty: When `False`, reject empty strings.
        max_length: Maximum allowed character count.
    """

    def _validator(value: object, instance: object | None = None) -> None:
        if not isinstance(value, str):
            raise TypeError(f"expected a string, got {type(value).__name__}")
        if not allow_empty and not value:
            raise ValueError("must not be empty")
        if max_length is not None and len(value) > max_length:
            raise ValueError(
                f"must be at most {max_length} characters, got {len(value)}"
            )
        illegal = _XML_ILLEGAL.search(value)
        if illegal is not None:
            raise ValueError(
                f"must not contain the control character {illegal.group()!r}: "
                "XML has no representation for it"
            )
        if _AMBIGUOUS_TAIL.search(value):
            raise ValueError(
                "must not end with a newline followed by tabs: the saved file "
                "would be read back differently by py and by Premiere"
            )
        try:
            value.encode("utf-8")
        except UnicodeEncodeError as error:
            raise ValueError(f"must be encodable as UTF-8: {error}") from None

    return _validator


def validate_path(
    *,
    must_exist: bool | None = None,
    must_be_file: bool = False,
) -> Callable[..., None]:
    """Return a validator that checks a filesystem path.

    Args:
        must_exist: When `True`, reject paths that don't exist
            (`ValueError`). When `False`, reject paths that do exist
            (`FileExistsError`). When `None`, allow both.
        must_be_file: When `True`, also reject an existing path that is not
            a regular file (a directory would otherwise fail later as an
            OS-specific error deep in a writer).
    """

    def _validator(value: object, instance: object | None = None) -> None:
        if not isinstance(value, (str, os.PathLike)):
            raise TypeError(f"expected a file system path, got {type(value).__name__}")
        path = Path(value)
        if must_exist is True:
            if not path.exists():
                raise ValueError(f"path does not exist: {path}")
            if must_be_file and not path.is_file():
                raise ValueError(f"path is not a file: {path}")
        elif must_exist is False and path.exists():
            raise FileExistsError(f"path already exists: {path}")

    return _validator


def validate_bool(value: object, instance: object | None = None) -> None:
    """Validate that a value is a `bool` (rejects 0/1 integers)."""
    if not isinstance(value, bool):
        raise TypeError(f"expected a bool, got {type(value).__name__}")


#: `Time` values are checked by `validate_time`, which lives in `time.py`
#: instead of here: this module is imported BY `time.py`, so it cannot name
#: the `Time` class.


def validate_vector2(value: object, instance: object | None = None) -> None:
    """Validate a 2-component numeric vector (a point).

    Accepts a list or tuple of exactly two finite numbers; rejects bools.
    """
    if not isinstance(value, (list, tuple)) or len(value) != 2:
        raise TypeError(f"expected a 2-tuple of numbers, got {type(value).__name__}")
    for component in value:
        if not isinstance(component, (int, float)) or isinstance(component, bool):
            raise TypeError(f"expected numbers, got {type(component).__name__}")
        if isinstance(component, float) and not math.isfinite(component):
            raise ValueError(f"must be finite numbers, got {component}")


def validate_color(value: object, instance: object | None = None) -> None:
    """Validate an RGBA `Color` (each channel an integer 0-255)."""
    if not isinstance(value, Color):
        raise TypeError(f"expected a Color, got {type(value).__name__}")
    for channel in value:
        if not isinstance(channel, int) or isinstance(channel, bool):
            raise TypeError(f"expected integer channels, got {type(channel).__name__}")
        if not 0 <= channel <= 255:
            raise ValueError(f"channel out of range 0-255: {channel}")


# ---- Shared domain validators ----
# Re-use these across models instead of defining per-module duplicates.

validate_number = _validate_number()

validate_positive_number = _validate_number(min=0.0)

validate_int = _validate_number(integer=True)

validate_positive_int = _validate_number(integer=True, min=0)

#: A sequence track count. At least one; no upper bound is imposed because
#: the format's own limit has not been measured.
validate_track_count = _validate_number(integer=True, min=1)

#: A project-item color-label index (Premiere's 16-colour palette, 0-15).
validate_color_label = _validate_number(integer=True, min=0, max=15)

#: A marker color index (Premiere's 8-colour marker palette, 0-7). Typed like
#: `validate_color_label` so `True` and `3.0` are refused, which plain set
#: membership would accept (`True == 1`).
validate_marker_color_index = _validate_number(integer=True, min=0, max=7)

#: The largest magnitude a float32 param value can hold; beyond it `struct`
#: raises `OverflowError` from deep inside the writer.
FLOAT32_MAX = 3.4028234663852886e38

#: A param value that must survive being packed as float32.
validate_float32 = _validate_number(min=-FLOAT32_MAX, max=FLOAT32_MAX)

#: A packed color param value (`0xAA00RR00GG00BB00` in a uint64).
validate_packed_color = _validate_number(integer=True, min=0, max=2**64 - 1)
