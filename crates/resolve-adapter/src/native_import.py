"""Read readiness or export one original from an already active Resolve worker."""
import fcntl
import json
import os
from pathlib import Path
import sys
import tempfile


READINESS_EXIT = {
    "worker_busy": 3,
    "worker_contains_content": 4,
    "worker_setup": 5,
    "worker_rendering": 6,
}


def connection(worker_name):
    try:
        import DaVinciResolveScript as api

        resolve = api.scriptapp("Resolve")
        if not resolve:
            return "disconnected", "", None
        product = resolve.GetProductName()
        version = resolve.GetVersion()[:4]
    except (ImportError, OSError, RuntimeError, AttributeError):
        return "disconnected", "", None
    if product == "DaVinci Resolve":
        return "free", "", None
    if product != "DaVinci Resolve Studio":
        return "disconnected", "", None
    if version != [21, 2, 0, 6]:
        return "unsupported_build", ".".join(map(str, version)), None
    if not worker_name.startswith("Any NLE Import Worker "):
        return "worker_unconfigured", "", None
    manager = resolve.GetProjectManager()
    project = manager.GetCurrentProject()
    if not project or project.GetName() != worker_name:
        return "open_worker", worker_name, None
    pool = project.GetMediaPool()
    folder = pool.GetRootFolder()
    if project.IsRenderingInProgress():
        return "worker_rendering", worker_name, None
    if project.GetTimelineCount() or folder.GetClipList() or folder.GetSubFolderList():
        return "worker_contains_content", worker_name, None
    if float(project.GetSetting("timelineFrameRate")) != 24:
        return "worker_setup", "Set the scratch project timeline rate to 24 fps.", None
    if pool.GetCurrentFolder().GetUniqueId() != folder.GetUniqueId():
        return "worker_setup", "Select the scratch project's Media Pool root folder.", None
    return "ready", "", (manager, project, pool)


def import_original(worker_name, original, destination):
    state, _, worker = connection(worker_name)
    if state != "ready":
        return READINESS_EXIT.get(state, 2)
    manager, project, pool = worker
    identity = project.GetUniqueId()

    def active():
        current = manager.GetCurrentProject()
        return current and current.GetUniqueId() == identity

    if not active():
        return 2
    clips = pool.ImportMedia([original]) or []
    try:
        if len(clips) != 1:
            return 1
        clip = clips[0]
        if (
            clip.GetClipProperty("Online Status") != "Online"
            or clip.GetClipProperty("File Path") != original
        ):
            return 1
        if not active():
            return 2
        return 0 if manager.ExportProject(worker_name, destination, False) else 1
    finally:
        if clips and not pool.DeleteClips(clips):
            raise RuntimeError("could not remove the imported clips from the scratch project")


def main():
    worker_name, *args = sys.argv[1:]
    lock = Path(tempfile.gettempdir()) / "anynle-resolve-import-worker.lock"
    if args == ["--status"]:
        # A status check never creates or holds the lock.
        try:
            with lock.open("rb") as lock_file:
                fcntl.flock(lock_file, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except FileNotFoundError:
            pass
        except BlockingIOError:
            print(json.dumps(["worker_busy", worker_name]))
            return 0
        state, detail, _ = connection(worker_name)
        print(json.dumps([state, detail]))
        return 0
    original, destination = args
    if not worker_name.startswith("Any NLE Import Worker "):
        return 2
    fd = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "r+") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return READINESS_EXIT["worker_busy"]
        return import_original(worker_name, original, destination)


if __name__ == "__main__":
    sys.exit(main())
