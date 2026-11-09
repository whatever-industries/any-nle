"""The `NamedList` collection view returned by model list properties."""

from __future__ import annotations

from typing import TYPE_CHECKING, List, TypeVar, overload

if TYPE_CHECKING:
    from typing import Iterable

T = TypeVar("T")


class NamedList(List[T]):
    """A list of model objects, also indexable by name.

    `collection["Motion"]` returns the first item whose name matches
    (raising `KeyError` when none does), `collection.get("Motion")` returns
    it or `None`, and `"Motion" in collection` tests for one. Integer
    indexes and slices behave like a plain list.

    The view is a snapshot: mutating it does not touch the model. Use the
    owning model's `add_*`/`remove_*` methods to change the project.
    """

    def __init__(
        self, items: Iterable[T] = (), keys: tuple[str, ...] = ("name",)
    ) -> None:
        super().__init__(items)
        self._keys = keys

    def _matches(self, item: T, name: str) -> bool:
        return any(getattr(item, key) == name for key in self._keys)

    # The `str` overload deliberately widens list's signature.
    @overload  # type: ignore[override]
    def __getitem__(self, key: int) -> T: ...

    @overload
    def __getitem__(self, key: slice) -> list[T]: ...

    @overload
    def __getitem__(self, key: str) -> T: ...

    def __getitem__(self, key: int | slice | str) -> T | list[T]:
        if isinstance(key, str):
            for item in self:
                if self._matches(item, key):
                    return item
            raise KeyError(key)
        return super().__getitem__(key)

    def get(self, name: str, default: T | None = None) -> T | None:
        """The first item named `name`, or `default` when there is none."""
        for item in self:
            if self._matches(item, name):
                return item
        return default

    def __contains__(self, item: object) -> bool:
        if isinstance(item, str):
            return any(self._matches(candidate, item) for candidate in self)
        return super().__contains__(item)
