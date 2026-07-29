"""Operator-history test doubles, shared by test_actions / test_redo_panel.

Kept out of conftest.py — that file is a pytest plugin, not an importable
module — so the shared fakes live here and conftest.py holds only fixtures.
conftest.py puts this directory on sys.path, so `import _op_helpers` works
regardless of pytest's import mode.
"""


class FakeOp:
    """Stand-in for a stored ``wm.operators`` history entry.

    ``kw=None`` models the real case where a stored entry's parameters are
    unreadable: ``as_keywords()`` raises rather than returning a dict.
    """

    def __init__(self, idname, name="", kw=None):
        self.bl_idname = idname
        self.name = name
        self._kw = kw

    def as_keywords(self):
        if self._kw is None:
            raise RuntimeError("params unavailable on stored history entry")
        return self._kw

    def as_pointer(self):
        return id(self)


class MacroSubOp:
    """Stand-in for a macro's sub-operator reference: has bl_rna but not
    as_keywords, per actions._is_macro_subop_ref."""

    bl_rna = object()


class StaleRnaValue:
    """Parameter value whose bl_rna access raises like a freed RNA struct.

    hasattr() only swallows AttributeError, so this is what escapes
    actions._is_macro_subop_ref if the sub-op scan is not itself guarded.
    """

    @property
    def bl_rna(self):
        raise ReferenceError("StructRNA of type Object has been removed")


def set_ops(fake_bpy, ops) -> None:
    """Replace the fake operator-history ring, oldest -> newest."""
    fake_bpy.context.window_manager.operators = ops
