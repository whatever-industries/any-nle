#!/usr/bin/env python3
"""Report a timeline from the current Resolve project through its scripting API."""

import argparse
import json


MAX_TRACKS = 256
MAX_ITEMS = 50_000


def snapshot(resolve, timeline_index=None):
    if not resolve:
        raise ValueError("Resolve scripting is unavailable")
    project = resolve.GetProjectManager().GetCurrentProject()
    if not project:
        raise ValueError("Resolve has no current project")
    if timeline_index is None:
        timeline = project.GetCurrentTimeline()
    else:
        if not 1 <= timeline_index <= project.GetTimelineCount():
            raise ValueError("timeline index is outside the current project")
        timeline = project.GetTimelineByIndex(timeline_index)
    if not timeline:
        raise ValueError("Resolve has no selected timeline for read-back")

    tracks = []
    item_count = 0
    for kind in ("video", "audio", "subtitle"):
        count = timeline.GetTrackCount(kind)
        if not isinstance(count, int) or not 0 <= count <= MAX_TRACKS - len(tracks):
            raise ValueError("Resolve track count exceeds inspection limit")
        for index in range(1, count + 1):
            source_items = timeline.GetItemListInTrack(kind, index)
            if not isinstance(source_items, list):
                raise ValueError("Resolve did not return a timeline item list")
            item_count += len(source_items)
            if item_count > MAX_ITEMS:
                raise ValueError("Resolve item count exceeds inspection limit")
            items = []
            for item in source_items:
                media = item.GetMediaPoolItem()
                items.append({
                    "api_id": item.GetUniqueId(),
                    "type": item.GetType(),
                    "start_frame": item.GetStart(True),
                    "end_frame": item.GetEnd(True),
                    "duration_frames": item.GetDuration(True),
                    "api_source_start_frame": item.GetSourceStartFrame(),
                    "api_source_end_frame": item.GetSourceEndFrame(),
                    "api_media_id": media.GetUniqueId() if media else None,
                })
            tracks.append({
                "kind": kind,
                "index": index,
                "name": timeline.GetTrackName(kind, index),
                "enabled": timeline.GetIsTrackEnabled(kind, index),
                "locked": timeline.GetIsTrackLocked(kind, index),
                "items": items,
            })
    return {
        "schema": "anynle.resolve-native-timeline/1",
        "resolve_version": resolve.GetVersion(),
        "timeline": {
            "api_id": timeline.GetUniqueId(),
            "start_timecode": timeline.GetStartTimecode(),
            "start_frame": timeline.GetStartFrame(),
            "end_frame": timeline.GetEndFrame(),
            "tracks": tracks,
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--timeline-index", type=int, help="one-based index in the current project")
    args = parser.parse_args()
    try:
        import DaVinciResolveScript

        report = snapshot(DaVinciResolveScript.scriptapp("Resolve"), args.timeline_index)
        print(json.dumps(report, indent=2, allow_nan=False))
    except (ImportError, AttributeError, RuntimeError, TypeError, ValueError) as error:
        parser.exit(1, f"Resolve read-back: {error}\n")


if __name__ == "__main__":
    main()
