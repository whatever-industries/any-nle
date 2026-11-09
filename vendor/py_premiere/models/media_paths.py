"""Lexical media paths: a Windows path must not become relative to a POSIX cwd."""
import os
from pathlib import PureWindowsPath


def relative_media_path(path: str, directory: str) -> str:
    # Absolute paths are the existing fallback for media on another drive. Keep that
    # fallback for Windows paths on every host, without interpreting them through cwd.
    if PureWindowsPath(path).is_absolute():
        return path
    try:
        return os.path.relpath(path, directory)
    except ValueError:
        return path
