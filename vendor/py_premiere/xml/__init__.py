"""Gzipped-XML document layer for `.prproj` files.

This layer owns file framing, parsing and byte-fidelity serialization; it
never imports from `models/`.
"""

from __future__ import annotations

from .document import PremiereDocument, parse_prproj

__all__ = ["PremiereDocument", "parse_prproj"]
