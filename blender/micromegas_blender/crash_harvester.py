"""
Crash file harvester — Phase 1 crash capture.

On Blender startup, scans the platform crash directory for *.crash.txt files
left by a prior abnormal exit.  Each file is claimed via atomic rename
(*.crash.txt → *.crash.txt.claimed) before upload, so multiple concurrent
Blender instances never double-report the same crash.

The crash report is shipped as a FATAL-level log keyed to the *current*
session's handle.  The last user actions before the crash are already in the
telemetry stream under the prior session's process fingerprint — no local
store needed.

Best-effort: if upload fails after claiming, the report is lost (no retry,
no recovery).  This is by design — Phase 1 measures how lossy this free path
is; if loss is unacceptable, Phase 2 (Crashpad minidumps) is pursued as a
separate initiative.
"""

import glob
import os
import platform
import sys

from . import binding as _b

_lib: "_b.MicromegasLib | None" = None
_handle = None

# Maximum crash file size to ship (guard against multi-MB core dumps).
_MAX_BYTES = 512 * 1024  # 512 KiB


def set_context(lib: "_b.MicromegasLib", handle) -> None:
    global _lib, _handle
    _lib, _handle = lib, handle


def _get_crash_dirs() -> list[str]:
    """Return the directories Blender writes crash files to, per platform."""
    dirs: list[str] = []
    if sys.platform == "win32":
        temp = os.environ.get("TEMP") or os.environ.get("TMP") or "C:\\Temp"
        dirs.append(temp)
    else:
        dirs.append("/tmp")
    # Blender also writes to the user temp dir.
    try:
        import tempfile
        dirs.append(tempfile.gettempdir())
    except Exception:
        pass
    return list(dict.fromkeys(dirs))  # deduplicate, preserve order


def _upload_crash(claimed_path: str) -> bool:
    """Ship the claimed crash file as a FATAL log.  Returns True on success."""
    if not _lib or not _handle:
        return False
    try:
        with open(claimed_path, "r", encoding="utf-8", errors="replace") as f:
            content = f.read(_MAX_BYTES)
        truncated = " [truncated]" if os.path.getsize(claimed_path) > _MAX_BYTES else ""
        msg = f"prior_crash_report{truncated}: {content}"
        _lib.log(_handle, _b.LEVEL_FATAL, "blender.crash", msg)
        _lib.flush(_handle)
        return True
    except Exception as exc:
        return False


def harvest() -> int:
    """Scan for crash files and ship them.  Returns the number reported."""
    reported = 0
    seen: set[str] = set()
    for crash_dir in _get_crash_dirs():
        for crash_file in glob.glob(os.path.join(crash_dir, "*.crash.txt")):
            real = os.path.realpath(crash_file)
            if real in seen:
                continue
            seen.add(real)
            claimed = crash_file + ".claimed"
            try:
                os.rename(crash_file, claimed)
            except OSError:
                # Another instance claimed it — skip.
                continue
            if _upload_crash(claimed):
                reported += 1
            # Whether upload succeeded or failed, leave the .claimed file in
            # place (it's already moved away from *.crash.txt so no re-harvest).
    return reported


def register_startup_harvest() -> None:
    """Wire harvest() into bpy.app.handlers.load_factory_startup_post."""
    try:
        import bpy  # only available inside Blender

        def _harvest_handler(scene=None, depsgraph=None):
            count = harvest()
            if count and _lib and _handle:
                _lib.log(
                    _handle,
                    _b.LEVEL_INFO,
                    "blender.crash",
                    f"harvested {count} prior crash report(s)",
                )

        if _harvest_handler not in bpy.app.handlers.load_factory_startup_post:
            bpy.app.handlers.load_factory_startup_post.append(_harvest_handler)
    except ImportError:
        pass


def unregister_startup_harvest() -> None:
    try:
        import bpy

        for fn in list(bpy.app.handlers.load_factory_startup_post):
            if getattr(fn, "__module__", "") == __name__:
                bpy.app.handlers.load_factory_startup_post.remove(fn)
    except ImportError:
        pass
