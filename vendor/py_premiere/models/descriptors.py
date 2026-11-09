"""The `XmlField` descriptor: a backing-element leaf as a typed attribute.

Reads decode from the backing element on every access (the XML tree is the
source of truth); writes validate, then write through to the element text so
`Project.save` persists them. Read-only attributes do not use this class -
they are plain `@property` without a setter.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Generic, TypeVar, overload

from ..xml.mutations import insert_leaf_before

if TYPE_CHECKING:
    import xml.etree.ElementTree as ET
    from typing import Any, Callable

T = TypeVar("T")


class XmlField(Generic[T]):
    """Expose the text of a leaf element as a read/write model attribute.

    Args:
        path: Element path relative to the backing element (e.g. `Name` or
            `ClipTrackItem/TrackItem/Start`).
        element_attr: Name of the instance attribute holding the backing
            element (some ExtendScript attributes live on a linked object,
            e.g. a track item's name is stored on its `SubClip`).
        transform: `text -> domain value`; `None` means the raw string.
        reverse: `domain value -> text`; `None` means the value itself
            (strings only).
        validate: `(value, instance) -> None` validator from
            `models/validators.py`, run before every write.
        default: Text assumed when the element is absent (Premiere elides
            zero-valued fields). Without it, a missing element is an error.
        insert_before: Sibling tag to anchor creation of an elided element
            on first write. Without it, writing to a missing element raises
            (element order is schema-relevant, so creation must be proven
            per field).
        after_write: `(instance) -> None`, run once the element holds the
            new text. For fields another part of the file caches, so the
            cache can follow the write.
    """

    def __init__(
        self,
        path: str,
        *,
        element_attr: str = "_element",
        transform: Callable[[str], T] | None = None,
        reverse: Callable[[T], str] | None = None,
        validate: Callable[..., None] | None = None,
        default: str | None = None,
        insert_before: str | None = None,
        after_write: Callable[[Any], None] | None = None,
    ) -> None:
        self.path = path
        self.element_attr = element_attr
        self.transform = transform
        self.reverse = reverse
        self.validate = validate
        self.default = default
        self.insert_before = insert_before
        self.after_write = after_write
        self.name = path

    def __set_name__(self, owner: type, name: str) -> None:
        self.name = name

    def _split(self) -> tuple[str, str]:
        head, _, leaf = self.path.rpartition("/")
        return head, leaf

    def _find(self, instance: object) -> tuple[ET.Element, ET.Element | None]:
        base: ET.Element = getattr(instance, self.element_attr)
        head, leaf = self._split()
        parent = base.find(head) if head else base
        if parent is None:
            raise ValueError(f"missing <{head}> under <{base.tag}>")
        return parent, parent.find(leaf)

    @overload
    def __get__(self, instance: None, owner: type) -> XmlField[T]: ...

    @overload
    def __get__(self, instance: object, owner: type | None = None) -> T: ...

    def __get__(self, instance: object | None, owner: type | None = None) -> Any:
        if instance is None:
            return self
        _, element = self._find(instance)
        if element is None:
            if self.default is None:
                raise ValueError(f"missing <{self.path}> element")
            text = self.default
        else:
            text = element.text or ""
        return self.transform(text) if self.transform is not None else text

    def __set__(self, instance: object, value: T) -> None:
        if self.validate is not None:
            self.validate(value, instance)
        text = self.reverse(value) if self.reverse is not None else value
        if not isinstance(text, str):
            raise TypeError(f"{self.name}: serialized value must be a string")
        parent, element = self._find(instance)
        if element is None:
            if self.insert_before is None:
                raise ValueError(
                    f"cannot set {self.name}: <{self.path}> is absent and its "
                    "creation anchor is not defined for this field"
                )
            _, leaf = self._split()
            insert_leaf_before(parent, self.insert_before, leaf, text)
        else:
            element.text = text
        if self.after_write is not None:
            self.after_write(instance)
