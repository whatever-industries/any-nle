"""Deterministic IDs for reference output (Rust port plan, Phase 0).

The Python writer is the oracle for the Rust writer: the same model must give byte-identical
XML. The only nondeterminism is `uuid.uuid4()` (ObjectUIDs, ClipIDs, FileKeys, BinaryHashes,
marker IDs, LUT IDs), always called through the `uuid` module, in py_premiere and here. Within
`seeded_ids()` it returns a counter-based version-4 UUID sequence instead. ObjectIDs already come
from the document's counter; gzip mtime is fixed at 0. Run with PYTHONHASHSEED=0 so set
iteration order cannot vary either.
"""
from __future__ import annotations

import contextlib
import hashlib
import uuid


@contextlib.contextmanager
def seeded_ids(seed: str = "any-nle"):
    counter = [0]
    original = uuid.uuid4

    def next_uuid() -> uuid.UUID:
        counter[0] += 1
        digest = hashlib.sha256(f"{seed}:{counter[0]}".encode()).digest()[:16]
        return uuid.UUID(bytes=digest, version=4)

    uuid.uuid4 = next_uuid
    try:
        yield
    finally:
        uuid.uuid4 = original
