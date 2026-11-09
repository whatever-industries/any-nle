"""py_premiere - A .prproj (Adobe Premiere Pro project) parser."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

try:
    import importlib.metadata as importlib_metadata
except ImportError:
    import importlib_metadata  # type: ignore[import,no-redef]  # Python 3.7

from .models import TICKS_PER_SECOND, Time
from .models.skeleton import build_empty_project
from .parsers import parse_application
from .xml import PremiereDocument, parse_prproj
from .xml.gzip_io import GzipFraming

if TYPE_CHECKING:
    from .models import Application

try:
    __version__ = importlib_metadata.version("py-premiere")
except importlib_metadata.PackageNotFoundError:
    # Running from a source tree without an installed distribution.
    __version__ = "0.0.0"


def parse(
    path: str | Path,
    *,
    preferences_path: str | Path | None = None,
) -> Application:
    """Parse a `.prproj` file and return an `Application` object.

    `preferences_path` optionally points at a Premiere `Adobe Premiere Pro
    Prefs` file to read machine defaults from (import label/still-duration
    defaults). When omitted, the most recently used profile under
    `~/Documents/Adobe/Premiere Pro/` is used, or factory constants on a
    machine Premiere never ran on.
    """
    path = Path(path)
    application = parse_application(parse_prproj(path), path)
    if preferences_path is not None:
        application.project._preferences_path = Path(preferences_path)
    return application


def new(*, preferences_path: str | Path | None = None) -> Application:
    """Create a new, empty project from scratch.

    The project is built from scratch rather than copied from a bundled
    file, so each one gets its own identifiers and carries nothing from the
    machine that captured a skeleton. It holds the 9 objects that carry real
    state; Premiere restores the rest - the compile-settings tree and the
    project-panel view state - from its own defaults when it first saves.

    Add bins, markers and clips, then `application.project.save(path)` to
    write it. The project reports the name `untitled.prproj` until saved.

    `preferences_path` behaves as in `parse`.
    """
    document = PremiereDocument(build_empty_project(), GzipFraming.default())
    application = parse_application(document, Path("untitled.prproj"))
    if preferences_path is not None:
        application.project._preferences_path = Path(preferences_path)
    return application


__all__ = ["TICKS_PER_SECOND", "Time", "__version__", "new", "parse"]
