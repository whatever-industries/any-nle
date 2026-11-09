"""New-project publication without replacing an incumbent path."""
import errno
import os
from pathlib import Path
import sys


def _exclusive_rename(stage: Path, target: Path) -> bool:
    if sys.platform != "darwin":
        return False
    import ctypes
    libc = ctypes.CDLL(None, use_errno=True)
    function = getattr(libc, "renamex_np", None)
    if function is None:
        return False
    function.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
    function.restype = ctypes.c_int
    if function(os.fsencode(stage), os.fsencode(target), 4) == 0:  # RENAME_EXCL
        return True
    error = ctypes.get_errno()
    if error in {errno.EINVAL, errno.ENOTSUP, errno.ENOSYS, errno.EOPNOTSUPP}:
        return False
    raise OSError(error, os.strerror(error), str(target))


def _remove_stage(stage: Path) -> None:
    stage.unlink()


def publish_new(stage: Path, target: Path) -> str | None:
    if _exclusive_rename(stage, target):
        return None
    # Refuse if neither exclusive rename nor hard links are supported. Never
    # replace a late file or symlink, even after a successful preflight.
    os.link(stage, target)
    try:
        _remove_stage(stage)
    except OSError as error:
        return f"project published at {target}; staging cleanup failed for {stage}: {error}"
    return None
