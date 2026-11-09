"""Export a parsed project to interchange formats.

A one-way layer: it reads the model and writes a DIFFERENT file format, so
unlike the rest of the package there is no byte-fidelity contract to hold -
the bar is that the target application reads back what the source project
said. Nothing here touches `xml/`, which serializes `.prproj` itself.
"""

from .fcpxml import export_fcpxml

__all__ = ["export_fcpxml"]
