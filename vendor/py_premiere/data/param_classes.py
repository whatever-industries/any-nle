"""Component-param ClassIDs whose value decoding is verified.

Derived empirically (2026-07-20) by comparing decoded `StartKeyframe` values
against ExtendScript `getValue()` across the whole ground-truth corpus:
these classes matched on every instance (sliders, angles, packed colors,
points, track selectors). Popup/menu and group-header classes store internal
indices or UI state that ExtendScript reports differently and stay
unverified until mapped.
"""

from __future__ import annotations

#: ClassID -> what the class holds (informational).
VERIFIED_VALUE_CLASS_IDS = {
    "fe47129e-6c94-4fc0-95d5-c056a517aaf3",  # scalar sliders/angles/checkboxes
    "0fde4e9f-f895-4ba3-b0fe-9a6feafda583",  # colors (packed uint64)
    "ca81d347-309b-44d2-acc7-1c572efb973c",  # 2D points (x:y)
    "2f2eb0a3-318c-4a93-99fc-f1d319edc864",  # track selectors
    "6e02e8bb-2569-46b2-8ab1-4ab11c43e9c8",  # boolean flags
    "a714635e-a628-4b27-9d59-77eba47dbc1a",  # audio scalars (Level/Balance/channels)
}
