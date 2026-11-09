"""Gzip framing for `.prproj` files.

Premiere writes a fixed 10-byte gzip header (flags=0, mtime=0, xfl=0; only
the OS byte varies by writer) followed by a stock zlib level-6 deflate
stream. Reproducing the file byte-for-byte therefore only requires keeping
the original header and compressing with zlib defaults. The round-trip test
suite is the tripwire for platforms whose zlib emits different bytes.
Headers with optional fields (FNAME etc., e.g. a file re-gzipped by CLI
tools) are parsed and preserved verbatim.
"""

from __future__ import annotations

import struct
import zlib

GZIP_MAGIC = b"\x1f\x8b"
# Current Premiere builds write OS byte 10 (older ones wrote 19).
DEFAULT_OS_BYTE = 10

_FHCRC = 0x02
_FEXTRA = 0x04
_FNAME = 0x08
_FCOMMENT = 0x10


class GzipFraming:
    """Raw gzip header of a parsed file, re-emitted verbatim on save."""

    def __init__(self, header: bytes) -> None:
        self.header = header

    @classmethod
    def default(cls) -> GzipFraming:
        # For py-created files (e.g. scripts/make_synthetic_sample.py):
        # mtime=0 and the current Premiere OS byte.
        header = (
            GZIP_MAGIC
            + bytes([8, 0])
            + struct.pack("<I", 0)
            + bytes([0, DEFAULT_OS_BYTE])
        )
        return cls(header)


def _header_length(data: bytes) -> int:
    if len(data) < 10:
        raise ValueError("truncated gzip header")
    flags = data[3]
    pos = 10
    try:
        if flags & _FEXTRA:
            (xlen,) = struct.unpack_from("<H", data, pos)
            pos += 2 + xlen
        if flags & _FNAME:
            pos = data.index(b"\x00", pos) + 1
        if flags & _FCOMMENT:
            pos = data.index(b"\x00", pos) + 1
    except (struct.error, ValueError):
        raise ValueError("truncated gzip header") from None
    if flags & _FHCRC:
        pos += 2
    if pos > len(data):
        raise ValueError("truncated gzip header")
    return pos


def decompress_prproj(data: bytes) -> tuple[bytes, GzipFraming | None]:
    """Return `(xml payload, framing)`; framing is None for uncompressed files."""
    if not data.startswith(GZIP_MAGIC):
        return data, None
    header_length = _header_length(data)
    decompressor = zlib.decompressobj(-15)
    try:
        xml = decompressor.decompress(data[header_length:])
    except zlib.error as exc:
        raise ValueError(f"corrupt deflate stream: {exc}") from exc
    trailer = decompressor.unused_data
    if len(trailer) < 8:
        raise ValueError("truncated gzip trailer")
    if len(trailer) > 8:
        # compress_prproj emits a single member; concatenated members would
        # be silently restructured on save, so refuse them up front.
        raise ValueError("multi-member gzip files are not supported")
    crc, isize = struct.unpack("<II", trailer)
    if crc != zlib.crc32(xml) or isize != len(xml) & 0xFFFFFFFF:
        raise ValueError("gzip CRC/length mismatch")
    return xml, GzipFraming(data[:header_length])


def compress_prproj(xml: bytes, framing: GzipFraming | None) -> bytes:
    """Frame an XML payload the way Premiere would have written it."""
    if framing is None:
        return xml
    compressor = zlib.compressobj(6, zlib.DEFLATED, -15)
    deflated = compressor.compress(xml) + compressor.flush()
    trailer = struct.pack("<II", zlib.crc32(xml), len(xml) & 0xFFFFFFFF)
    return framing.header + deflated + trailer
