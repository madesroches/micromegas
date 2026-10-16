"""
ctypes binding to libmicromegas_capi.{so,dll}.

Loads the prebuilt cdylib that lives next to this file under lib/ and exposes
thin Python wrappers for each C function.  Everything here is pure ctypes —
no bpy dependency — so it can be imported and unit-tested outside Blender.

Level constants mirror micromegas.h:
    LEVEL_FATAL=1, LEVEL_ERROR=2, LEVEL_WARN=3,
    LEVEL_INFO=4,  LEVEL_DEBUG=5, LEVEL_TRACE=6
"""

import ctypes
import os
import sys
from typing import Optional

LEVEL_FATAL = 1
LEVEL_ERROR = 2
LEVEL_WARN = 3
LEVEL_INFO = 4
LEVEL_DEBUG = 5
LEVEL_TRACE = 6


class MmConfig(ctypes.Structure):
    _fields_ = [
        ("sink_url", ctypes.c_char_p),
        ("property_keys", ctypes.POINTER(ctypes.c_char_p)),
        ("property_values", ctypes.POINTER(ctypes.c_char_p)),
        ("property_count", ctypes.c_uint),
    ]


def _property_arrays(properties: Optional[dict]):
    """Parallel key/value arrays and their count, as the C functions take them."""
    props = properties or {}
    count = len(props)
    if not count:
        return None, None, 0
    keys_arr = (ctypes.c_char_p * count)(*(k.encode() for k in props.keys()))
    vals_arr = (ctypes.c_char_p * count)(*(v.encode() for v in props.values()))
    return keys_arr, vals_arr, count


def _get_lib_path() -> str:
    addon_dir = os.path.dirname(os.path.abspath(__file__))
    lib_dir = os.path.join(addon_dir, "lib")
    if sys.platform == "win32":
        return os.path.join(lib_dir, "micromegas_capi.dll")
    else:
        return os.path.join(lib_dir, "libmicromegas_capi.so")


class MicromegasLib:
    """Loaded cdylib with typed function signatures."""

    def __init__(self, lib_path: Optional[str] = None):
        path = lib_path or _get_lib_path()
        self._lib = ctypes.CDLL(path)
        self._configure_signatures()

    def _configure_signatures(self) -> None:
        lib = self._lib

        lib.mm_init.restype = ctypes.c_void_p
        lib.mm_init.argtypes = [ctypes.POINTER(MmConfig)]

        lib.mm_shutdown.restype = None
        lib.mm_shutdown.argtypes = [ctypes.c_void_p]

        lib.mm_log.restype = None
        lib.mm_log.argtypes = [
            ctypes.c_void_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_char_p,
        ]

        lib.mm_log_tagged.restype = None
        lib.mm_log_tagged.argtypes = [
            ctypes.c_void_p,
            ctypes.c_int,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.c_uint,
        ]

        lib.mm_metric_i.restype = None
        lib.mm_metric_i.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.c_uint64,
        ]

        lib.mm_metric_f.restype = None
        lib.mm_metric_f.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.c_double,
        ]

        lib.mm_metric_i_tagged.restype = None
        lib.mm_metric_i_tagged.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.c_uint,
            ctypes.c_uint64,
        ]

        lib.mm_metric_f_tagged.restype = None
        lib.mm_metric_f_tagged.argtypes = [
            ctypes.c_void_p,
            ctypes.c_char_p,
            ctypes.c_char_p,
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.POINTER(ctypes.c_char_p),
            ctypes.c_uint,
            ctypes.c_double,
        ]

        lib.mm_flush.restype = None
        lib.mm_flush.argtypes = [ctypes.c_void_p]

    # ------------------------------------------------------------------
    # Public wrappers
    # ------------------------------------------------------------------

    def init(
        self,
        sink_url: Optional[str] = None,
        properties: Optional[dict] = None,
    ) -> Optional[ctypes.c_void_p]:
        """Initialize telemetry and return an opaque handle, or None on failure."""
        url_bytes = sink_url.encode() if sink_url else None

        keys_arr, vals_arr, count = _property_arrays(properties)
        cfg = MmConfig(
            sink_url=url_bytes,
            property_keys=keys_arr,
            property_values=vals_arr,
            property_count=count,
        )

        handle = self._lib.mm_init(ctypes.byref(cfg))
        return handle if handle else None

    def shutdown(self, handle) -> None:
        if handle:
            self._lib.mm_shutdown(handle)

    def log(
        self,
        handle,
        level: int,
        target: str,
        msg: str,
        properties: Optional[dict] = None,
    ) -> None:
        if not handle:
            return
        if properties:
            keys_arr, vals_arr, count = _property_arrays(properties)
            self._lib.mm_log_tagged(
                handle, level, target.encode(), msg.encode(), keys_arr, vals_arr, count
            )
        else:
            self._lib.mm_log(handle, level, target.encode(), msg.encode())

    def metric_i(
        self,
        handle,
        name: str,
        unit: str,
        value: int,
        properties: Optional[dict] = None,
    ) -> None:
        if not handle:
            return
        if properties:
            keys_arr, vals_arr, count = _property_arrays(properties)
            self._lib.mm_metric_i_tagged(
                handle, name.encode(), unit.encode(), keys_arr, vals_arr, count, int(value)
            )
        else:
            self._lib.mm_metric_i(handle, name.encode(), unit.encode(), int(value))

    def metric_f(
        self,
        handle,
        name: str,
        unit: str,
        value: float,
        properties: Optional[dict] = None,
    ) -> None:
        if not handle:
            return
        if properties:
            keys_arr, vals_arr, count = _property_arrays(properties)
            self._lib.mm_metric_f_tagged(
                handle, name.encode(), unit.encode(), keys_arr, vals_arr, count, float(value)
            )
        else:
            self._lib.mm_metric_f(handle, name.encode(), unit.encode(), float(value))

    def flush(self, handle) -> None:
        if handle:
            self._lib.mm_flush(handle)
