"""The `Application` model."""

from __future__ import annotations

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from .project import Project


class Application:
    """The Premiere Pro application object (`app` in ExtendScript)."""

    def __init__(self, project: Project) -> None:
        self._project = project

    @property
    def project(self) -> Project:
        """The active project. Read-only."""
        return self._project

    def __repr__(self) -> str:
        return f"Application({self._project!r})"
