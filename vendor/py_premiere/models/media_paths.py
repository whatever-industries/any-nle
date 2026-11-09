"""Lexical media paths: a Windows path must not become relative to a POSIX cwd."""
import ntpath
import os
import posixpath
from pathlib import PureWindowsPath


def media_basename(path: str) -> str:
    if PureWindowsPath(path).is_absolute():
        return ntpath.basename(path)
    return posixpath.basename(path)


def relative_media_path(path: str, directory: str) -> str:
    # Absolute paths are the existing fallback for media on another drive. Keep that
    # fallback for Windows paths on every host, without interpreting them through cwd.
    if PureWindowsPath(path).is_absolute():
        return path
    try:
        return os.path.relpath(path, directory)
    except ValueError:
        return path
