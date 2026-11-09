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
    # Use the absolute reference across drives or path styles. Preserve the
    # recorded Windows reference independently of the host's working directory.
    if PureWindowsPath(path).is_absolute() or (
        path.startswith("/") and PureWindowsPath(directory).is_absolute()
    ):
        return path
    try:
        return os.path.relpath(path, directory)
    except ValueError:
        return path
