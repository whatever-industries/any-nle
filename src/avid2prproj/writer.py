"""Write a native Premiere project from the neutral edit model.

Spike scope: bin mode "separate" (one item per OP-Atom file), linked timeline placement
at exact ticks, comment markers. "grouped" and "merged" are refused until their Premiere
shapes are observed in User fixtures.
"""
from __future__ import annotations

from dataclasses import dataclass, field, replace
from fractions import Fraction
from itertools import chain
from pathlib import Path

import py_premiere
from py_premiere import Time
from py_premiere.models import sequence_builder

from . import colors, metadata
from . import markers as mk
from . import avid_color
from .lumetri import apply_lumetri_lut, write_cube
from .matte import apply_matte
from .motion import apply_motion

LUT_SIZE = 33
from .combined import (DISPLAY_FORMATS, _move_to_processed_clips, _parts_bin, create_grouped, create_merged,
                       create_stereo_nest, declare_stereo_chain, group_for_channel, narrow_to_group, set_clip_pan)
from .placement import normalize_placement
from .audio import add_crossfade, set_clip_gain, set_clip_volume, set_track_level, split_overflow
from .model import event_audio_files, Clip, Marker, Model, Sequence, audio_for_combined_clip, ticks_per_unit
from .opatom import UnverifiedShape, add_audio_item, add_video_item, source_item_names

#: Sequence formats beyond py_premiere's verified presets, keyed by (rate, width, height).
#: 24p: time display 100 (the TimecodeFormat Premiere gave 24 fps media, PRPROJ-0009);
#: User read-back in Premiere 26.5.2 showed correct 24 fps timecode (PRPROJ-0010, CONV-0006).
EXTRA_FORMATS = {"avid2prproj-1080p24": sequence_builder.SequenceFormat(10584000000, 1920, 1080, 100)}


@dataclass
class Report:
    notes: list[str] = field(default_factory=list)
    unverified: list[str] = field(default_factory=list)

    def as_dict(self) -> dict:
        return {"notes": self.notes, "unverified": self.unverified}


@dataclass(frozen=True)
class _Format:
    """The fields `_preset` reads, for sequences the converter makes itself (stereo builds)."""
    name: str
    rate: Fraction
    width: int
    height: int


def _preset(seq: Sequence, report: Report, rewrapped=False) -> str:
    for name, fmt in sequence_builder.FORMATS.items():
        if fmt.frame_rate == ticks_per_unit(seq.rate) and (fmt.width, fmt.height) == (seq.width, seq.height) \
                and name in _VERIFIED_PRESETS:
            return name
    for name, fmt in EXTRA_FORMATS.items():
        if fmt.frame_rate == ticks_per_unit(seq.rate) and (fmt.width, fmt.height) == (seq.width, seq.height):
            sequence_builder.FORMATS.setdefault(name, fmt)
            report.notes.append(f"sequence {seq.name!r}: converter format {name} (User-reported in Premiere 26.5.2, PRPROJ-0010)")
            return name
    if rewrapped:
        from .rewrap import display_format
        code=display_format(seq.rate)
        if not (0<seq.width<=16384 and 0<seq.height<=16384): raise ValueError('unsupported rewrap sequence format')
        name=f"anynle-op1a/{ticks_per_unit(seq.rate)}/{seq.width}/{seq.height}/{code}"
        sequence_builder.FORMATS.setdefault(name,sequence_builder.SequenceFormat(ticks_per_unit(seq.rate),seq.width,seq.height,code))
        report.unverified.append(f"sequence {seq.name!r}: opt-in OP1a format {seq.rate} fps {seq.width}x{seq.height}; native acceptance pending")
        return name
    raise UnverifiedShape(f"sequence {seq.name!r}: no sequence format for {seq.rate} fps {seq.width}x{seq.height}")


_VERIFIED_PRESETS = {"1080p23976", "1080p25", "1080p2997", "1080p50", "1080p5994"}


def set_playback_speed(placed, ratio: tuple[int, int]) -> None:
    """Constant speed on a placed clip: <PlaybackSpeed> (source frames per timeline frame) on the
    clip core, <PlayBackwards>true for reverse, as py_premiere reads them (verified there against
    ExtendScript); the in/out points written afterwards are stored scaled by the speed. Element
    position after OutPoint is UNVERIFIED (awaiting User read-back)."""
    import xml.etree.ElementTree as ET
    timeline, source = ratio
    core = placed._clip_element.find("Clip")
    speed = source / abs(timeline)
    for tag, text in (("PlaybackSpeed", repr(float(speed))), ("PlayBackwards", "true" if timeline < 0 else None)):
        leaf = core.find(tag)
        if text is None:
            continue
        if leaf is None:
            leaf = ET.SubElement(core, tag)
            previous = core[-2]
            leaf.tail, previous.tail = previous.tail, core[0].tail if len(core) else None
        leaf.text = text


def _report_stereo_pans(report: Report, seq: Sequence, i: int, track: int, pans) -> None:
    """Avid can pan each channel of a stereo edit; a Premiere stereo clip has one Balance.
    Default (unset or 100L/100R) converts exactly; anything else is reported for now."""
    if any(p is not None for p in pans) and not (pans[0] in (None, -100.0) and pans[-1] in (None, 100.0)):
        shown = "/".join("default" if p is None else f"{p:+.0f}" for p in pans)
        report.unverified.append(f"sequence {seq.name!r} event {i}: stereo edit on A{track} has per-channel pan {shown} "
                                 f"in Avid; placed as plain stereo (decision pending)")


def _timecode(ticks: int, rate: Fraction, start: int = 0) -> str:
    unit = ticks_per_unit(rate)
    fps = round(rate)
    frames = (ticks + start) // unit
    return f"{frames // (3600 * fps) % 24:02d}:{frames // (60 * fps) % 60:02d}:{frames // fps % 60:02d}:{frames % fps:02d}"


def _scoped(markers, tracks) -> list[Marker]:
    """The clip markers that sit on the given Avid master tracks ("V1", "A2"); None = all."""
    return [m for m in markers if tracks is None or m.track in tracks]


def _order_track_items(project, sequence) -> None:
    """Premiere lists a track's clips in time order; out of order (filler adjustment layers
    added after the edits), Premiere 26.5 played a V1 clip but did not draw it (User,
    2026-10-15)."""
    document = project._document
    for track in list(sequence.video_tracks) + list(sequence.audio_tracks):
        items = track._element.find("ClipTrack/ClipItems/TrackItems")
        if items is None:
            continue
        refs = items.findall("TrackItem")
        start = lambda ref: int(document.by_object_id[ref.get("ObjectRef")].findtext(".//ClipTrackItem/TrackItem/Start") or 0)  # noqa: E731
        ordered = sorted(refs, key=start)
        if ordered == refs:
            continue
        tails = [r.tail for r in refs]
        for r in refs:
            items.remove(r)
        for index, (r, tail) in enumerate(zip(ordered, tails)):
            r.set("Index", str(index))
            r.tail = tail
            items.append(r)
        track._clips.sort(key=lambda c: c.start.ticks)


def _label_items(project, clip: Clip, items, options) -> None:
    if clip.avid is None or clip.avid.color_rgb16 is None:
        return
    index = colors.nearest_label(clip.avid.color_rgb16, dict(options.clip_labels))
    for item in items:
        if item is not None:
            mk.set_label(project, item, index)


def _destination_layout(layout, bins):
    """Validate optional panel hierarchy independently of the Rust writer.

    `parents` is aligned with the selected input bins. Only these paths are
    materialized; the hierarchy is not a filesystem path or a model/1 field.
    """
    if layout is None:
        return None
    if not isinstance(layout, dict) or set(layout) != {"root_name", "parents"}:
        raise ValueError("invalid destination bin layout")
    parents = layout["parents"]
    if not isinstance(parents, (list, tuple)) or len(parents) != len(bins):
        raise ValueError("destination bin layout must match input bin count")
    def component(value):
        import unicodedata
        if not isinstance(value, str) or not value.strip() or len(value.encode("utf-8")) > 4096 or value in (".", "..") or any(c in "/\\" or unicodedata.category(c) == "Cc" or c in "\ufffe\uffff" for c in value):
            raise ValueError("invalid destination bin layout component")
        return len(value.encode("utf-8"))
    total = 64 + component(layout["root_name"])
    for path, source_bin in zip(parents, bins):
        if not isinstance(path, (list, tuple)) or len(path) > 62:
            raise ValueError("destination bin layout exceeds 64 levels")
        total += 64 + 128 + component(source_bin.name)
        for value in path:
            total += 128 + component(value)
            if total > 64 << 20:
                raise ValueError("destination bin layout exceeds 64 MiB")
        if total > 64 << 20:
            raise ValueError("destination bin layout exceeds 64 MiB")
    return layout


def write_project(model: Model, out_path: Path, *, media_relinks=(), native_op1a_master=False, destination_layout=None) -> Report:
    destination_layout = _destination_layout(destination_layout, model.bins)
    media_relinks = tuple(media_relinks)
    rewrapped = bool(media_relinks)
    from . import native_av
    if not native_op1a_master and any(c.multichannel_groups for c in model.clips.values()):
        raise native_av.invalid("multichannel groups require the explicit native media route")
    if native_op1a_master:
        native_av.validate_bindings(model,media_relinks)
    from .rewrap import validate_audio_formats
    validate_audio_formats(model, media_relinks)
    out_path = Path(out_path)
    if out_path.exists():
        raise FileExistsError(f"{out_path} exists; the converter never overwrites")
    if not out_path.parent.is_dir():
        raise FileNotFoundError(f"output folder {out_path.parent} does not exist")
    report = Report()
    report.notes.append(f"bin mode: {model.options.bin_mode}")
    report.notes.append({"separate": "timeline: linked source clips", "merged": "timeline: merged clips (placed as their linked components)",
                         "grouped": "timeline: grouped (multicam) clips"}[model.options.bin_mode])

    if native_op1a_master:
        report.notes = ["clip construction: native OP1a-backed AV masters; explicit route replaces bin mode",
                        "timeline: native source groups routed from the Avid sequence"]
        report.unverified.append("Native OP1a AV masters: profiles beyond retained 23.976 fps checks with two PCM16/24 or three PCM24 mono streams require Premiere read-back; User acceptance remains pending")
    if native_op1a_master and any(m['target_path'].lower().endswith('.mov') for m in media_relinks):
        report.notes[0]="clip construction: native prepared-media AV masters (DNx OP1a / ProRes MOV); explicit route replaces bin mode"
        report.unverified[0]="Native ProRes MOV AV masters: bounded square-pixel 1080p24 LT/PCM16/24 research read-back; wider profiles and User acceptance remain pending"
    if native_op1a_master:
        for c in model.clips.values():
            for group in c.multichannel_groups:
                destination="Premiere 5.1 L,R,C,LFE,Ls,Rs" if group.format=='5.1' else "8-channel Adaptive, Avid L,C,R,Lss,Rss,Lsr,Rsr,LFE order; no automatic 7.1 speaker bus"
                report.notes.append(f"clip {c.name!r}: {group.format} group {list(group.channels)} -> {destination}")
    app = py_premiere.new()
    project = app.project
    from .rewrap import video_format, display_format
    bound={m['source_path'] for m in media_relinks if m.get('audio_stream') is None}
    project._any_nle_mono_views={c.video.path for c in model.clips.values() if c.video and len(c.audio)>2 and any(m['source_path']==c.video.path and m['target_path'].lower().endswith('.mov') for m in media_relinks)} if native_op1a_master else set()
    project._any_nle_rewrap_formats={c.video.path:video_format(c) for c in model.clips.values() if c.video and c.video.path in bound}
    format_clips = sorted(model.clips.values(),key=lambda c:c.id) if native_op1a_master else model.clips.values()
    for c in format_clips:
        if c.video and project._any_nle_rewrap_formats.get(c.video.path)=="109":
            report.unverified.append(f"clip {c.name!r}: extended {c.video.rate} fps uses frame-count display; native high-rate timecode display remains unqualified")
    project._path = out_path  # relative media paths are computed against the output location
    root = project.root_item
    if destination_layout is not None:
        root = root.add_bin(destination_layout["root_name"])
    items: dict[str, tuple] = {}  # clip id -> (video item | None, [audio items])
    placement: dict[str, tuple] = {}  # clip id -> ("separate"|"merged"|"grouped", payload)
    clip_bins: dict[str, object] = {}  # clip id -> the Premiere bin it was imported into
    options = model.options

    def part_markers(clip: Clip, item, track: str) -> None:
        """Master-clip markers on a source item: those on its Avid track. Counted from the clip's
        first frame: written at media start timecode + offset, a marker on an MXF starting at
        01:00:00:00 did not appear in Premiere 26.5 (User, 2026-10-15)."""
        for m in _scoped(clip.markers, {track}):
            mk.add_clip_marker(item, m, m.start, options, report)

    def build_markers(clip: Clip, build, tracks) -> None:
        """Master-clip markers on a build (its own time starts at 0)."""
        for m in _scoped(clip.markers, tracks):
            mk.add_clip_marker(build, m, m.start, options, report)

    def import_clip(clip_id: str, target) -> None:
        clip = model.clips[clip_id]
        previous, project._import_target = project._import_target, target
        if native_op1a_master:
            try:
                item = native_av.add_item(project,clip,clip.name)
            finally:
                project._import_target = previous
            metadata.apply(project,item,clip.avid)
            _label_items(project,clip,[item],options)
            build_markers(clip,item,None)
            items[clip_id] = (item,[])
            clip_bins[clip_id] = target
            placement[clip_id] = ('native',item)
            return
        try:
            video_name, audio_names = source_item_names(clip)
            video = add_video_item(project, clip.video, clip, video_name) if clip.video else None
            audio = [add_audio_item(project, a, clip, n) for a, n in zip(clip.audio, audio_names)]
            for item in ([video] if video is not None else []) + audio:
                metadata.apply(project, item, clip.avid)
            _label_items(project, clip, [video, *audio], options)
            if video is not None:
                part_markers(clip, video, "V1")
            for item, f in zip(audio, clip.audio):
                part_markers(clip, item, f"A{f.avid_track}")
            if any(a.avid_track is None for a in clip.audio):
                report.notes.append(f"clip {clip.name!r}: audio without an Avid track was numbered by model position")
        finally:
            project._import_target = previous
        items[clip_id] = (video, audio)
        clip_bins[clip_id] = target
        placement[clip_id] = ("separate", (video, audio, {}))
        mode = model.options.bin_mode
        if mode == "merged" and clip.stereo_pairs and clip.video and target is not None:
            # Premiere cannot merge into stereo (Adobe); build the stereo pair as a group (User).
            mode = "grouped"
            report.notes.append(f"clip {clip.name!r}: Avid stereo pair; built as a group instead of merged")
        if mode != "separate" and target is not None and clip.video and clip.audio:
            order = {id(a): i for i, a in enumerate(clip.audio)}
            combined = [audio[order[id(a)]] for a in audio_for_combined_clip(clip)]
            if mode == "merged":
                merged_item, video_copy, audio_copies = create_merged(project, clip, video, combined, target, clip.name)
                metadata.apply(project, merged_item, clip.avid)
                _label_items(project, clip, [merged_item, video_copy, *audio_copies], options)
                if clip.markers:
                    build_markers(clip, merged_item, None)
                    report.unverified.append(f"clip {clip.name!r}: markers on a merged clip (time base unverified)")
                placement[clip_id] = ("merged", (video_copy, audio_copies, {}))
            else:
                grouped_item = create_grouped(project, clip, video, combined, target, clip.name)
                metadata.apply(project, grouped_item, clip.avid)
                _label_items(project, clip, [grouped_item], options)
                build_markers(clip, grouped_item, None)
                placement[clip_id] = ("grouped", grouped_item)
        elif target is not None and clip.stereo_pairs:
            # No combined clip (Separate mode, or audio-only): each Avid stereo pair becomes an
            # audio-only stereo build; its two mono files become its parts.
            stereo = {}
            only_pair = clip.video is None and len(clip.audio) == 2
            for left, right in clip.stereo_pairs:
                files = [next(f for f in clip.audio if f.avid_track == t) for t in (left, right)]
                pair_items = [audio[clip.audio.index(f)] for f in files]
                part = replace(clip, video=None, audio=tuple(files), stereo_pairs=((left, right),))
                name = clip.name if only_pair else f"{clip.name}_A{left:02d}A{right:02d}"
                preset = _preset(_Format(name, part.rate or Fraction(24), 1920, 1080), report, rewrapped)
                build = create_stereo_nest(project, pair_items, target, name, preset)
                metadata.apply(project, build, clip.avid)
                _label_items(project, clip, [build], options)
                build_markers(clip, build, {f"A{left}", f"A{right}"})
                stereo[(left, right)] = build
            placement[clip_id] = ("separate", (video, audio, stereo))
            if mode != "separate":
                report.notes.append(f"clip {clip.name!r}: no picture to {mode}; stereo pair built as a stereo audio clip")
        elif mode != "separate":
            report.notes.append(f"clip {clip.name!r}: not {mode} (needs video and audio, and a bin); placed as separate clips")
        if clip.material_umid is None:
            report.notes.append(f"clip {clip.name!r}: no material UMID; ClipID left empty")

    folders = {}
    for index, b in enumerate(model.bins):
        parent = root
        if destination_layout is not None:
            for component in destination_layout["parents"][index]:
                key = (id(parent), component)
                if key not in folders:
                    folders[key] = parent.add_bin(component)
                parent = folders[key]
        target = parent.add_bin(b.name)
        for clip_id in b.clips:
            if clip_id in items:
                report.notes.append(f"clip {clip_id!r} listed in several bins; imported once into the first")
                continue
            import_clip(clip_id, target)
    for seq in model.sequences:
        for e in seq.events:
            if e.clip not in items:
                import_clip(e.clip, root if destination_layout is not None else None)
                report.notes.append(f"clip {e.clip!r} used in a sequence but in no bin; imported at the root")

    def settle(e, source_ms, own_ms, extra, where: str):
        """Avid both a source marker (copied in at cut-in) and the edit's own marker on one frame:
        the winner stays on the clip; the other is dropped or goes to the sequence (options)."""
        def timeline(src: int) -> int:
            along = src - e.source_in
            if e.speed_ratio is not None:
                along = along * abs(e.speed_ratio[0]) // e.speed_ratio[1]
            return e.record_start + along
        own_frames, source_frames = {o.source for o in own_ms}, {m.start for m in source_ms}
        edit_wins = options.marker_conflict_winner == "edit"
        keep_source, keep_own = [], []
        for m in source_ms:
            if edit_wins and m.start in own_frames:
                if options.marker_conflict_loser_to_sequence:
                    extra.append((m, timeline(m.start), "clip marker replaced on this edit"))
            else:
                keep_source.append(m)
        for o in own_ms:
            if not edit_wins and o.source in source_frames:
                if options.marker_conflict_loser_to_sequence:
                    extra.append((o, o.start, "edit marker under a clip marker"))
            else:
                keep_own.append(o)
        if len(keep_source) + len(keep_own) < len(source_ms) + len(own_ms):
            report.notes.append(f"{where}: an edit marker and a clip marker share a frame; the {options.marker_conflict_winner} marker "
                                f"is kept{', the other placed on the sequence' if options.marker_conflict_loser_to_sequence else ''}")
        return keep_source, keep_own

    def marked(item, source_ms, own_ms, base: int, e, extra) -> None:
        keep_source, keep_own = settle(e, source_ms, own_ms, extra, f"{item.name!r}")
        for m in keep_source:
            mk.add_clip_marker(item, m, base + m.start, options, report)
        for o in keep_own:
            mk.add_clip_marker(item, o, base + o.source, options, report)

    def edit_copy(seq, i, e, kind, payload, own, extra):
        """Premiere keeps markers on the master clip, so an edit with markers of its own plays a
        copy of the clip carrying the source markers plus its own (User decision 2026-10-15).
        Returns the payload this edit is placed from."""
        clip = model.clips[e.clip]
        target = clip_bins.get(e.clip) or root
        name = f"{clip.name} [edit {_timecode(e.record_start, seq.rate, seq.start)}]"
        video, audio = items[e.clip]
        if kind == "native":
            previous, project._import_target = project._import_target, target
            try:
                copy = native_av.add_item(project,clip,name)
            finally:
                project._import_target = previous
            metadata.apply(project,copy,clip.avid)
            _label_items(project,clip,[copy],options)
            marked(copy,list(clip.markers),own,0,e,extra)
            _move_to_processed_clips(project,[copy],target)
            report.notes.append(f"sequence {seq.name!r} event {i}: edit markers; placed from its own copy {name!r}")
            return copy
        if kind == "merged":
            for o in own:
                extra.append((o, o.start, f"on this {o.track} edit (merged clips cannot carry per-edit markers yet)"))
            return payload
        if kind == "grouped":
            order = {id(a): j for j, a in enumerate(clip.audio)}
            combined = [audio[order[id(a)]] for a in audio_for_combined_clip(clip)]
            copy = create_grouped(project, clip, video, combined, target, name)
            metadata.apply(project, copy, clip.avid)
            _label_items(project, clip, [copy], options)
            marked(copy, list(clip.markers), own, 0, e, extra)
            _move_to_processed_clips(project, [copy], target)
            report.notes.append(f"sequence {seq.name!r} event {i}: edit markers; placed from its own copy {name!r}")
            return copy
        video_item, audio_items, stereo_builds = payload
        audio_items, stereo_builds = list(audio_items), dict(stereo_builds)
        parts = _parts_bin(target)
        files = event_audio_files(clip, e)
        stereo_tracks = {t: chans for t, chans, *_rest in e.stereo}
        previous, project._import_target = project._import_target, parts
        try:
            for track in sorted({o.track for o in own}):
                mine = [o for o in own if o.track == track]
                number = int(track[1:])
                if track.startswith("V") and number == e.video_track and video_item is not None:
                    video_item = add_video_item(project, clip.video, clip, f"{name}_V")
                    metadata.apply(project, video_item, clip.avid)
                    _label_items(project, clip, [video_item], options)
                    marked(video_item, _scoped(clip.markers, {"V1"}), mine, 0, e, extra)
                elif track.startswith("A") and number in e.audio_tracks:
                    f = files[e.audio_tracks.index(number)]
                    # The model permits sound without an Avid label. Use the same positional
                    # fallback as source_item_names, including for source-marker scope.
                    source_track = f.avid_track or clip.audio.index(f) + 1
                    copy = add_audio_item(project, f, clip, f"{name}_A{source_track:02d}")
                    metadata.apply(project, copy, clip.avid)
                    _label_items(project, clip, [copy], options)
                    marked(copy, _scoped(clip.markers, {f"A{source_track}"}), mine, 0, e, extra)
                    audio_items[clip.audio.index(f)] = copy
                elif track.startswith("A") and number in stereo_tracks and tuple(stereo_tracks[number]) in stereo_builds:
                    pair = tuple(stereo_tracks[number])
                    pair_items = [items[e.clip][1][clip.audio.index(next(f for f in clip.audio if f.avid_track == t))] for t in pair]
                    preset = _preset(_Format(name, clip.rate or Fraction(24), 1920, 1080), report, rewrapped)
                    copy = create_stereo_nest(project, pair_items, target, f"{name}_A{pair[0]:02d}A{pair[1]:02d}", preset)
                    metadata.apply(project, copy, clip.avid)
                    _label_items(project, clip, [copy], options)
                    marked(copy, _scoped(clip.markers, {f"A{t}" for t in pair}), mine, 0, e, extra)
                    _move_to_processed_clips(project, [copy], target)
                    stereo_builds[pair] = copy
                else:
                    for o in mine:
                        extra.append((o, o.start, f"on {o.track}"))
                    continue
                report.notes.append(f"sequence {seq.name!r} event {i}: {track} edit markers; placed from its own copy {name!r}")
        finally:
            project._import_target = previous
        return video_item, audio_items, stereo_builds

    for seq in model.sequences:
        preset = _preset(seq, report, rewrapped)
        if native_op1a_master:
            width=native_av.sequence_channels(model,seq)
            if width:
                from py_premiere.models.sequence_builder import FORMATS
                base = FORMATS[preset]
                preset = f"anynle-grouped/{width}/{preset}"
                FORMATS[preset] = base._replace(grouped_channels=width)
        needed = max(chain(
            (t for e in seq.events for t in e.audio_tracks),
            (s[0] for e in seq.events for s in e.stereo),
            (t.track for t in seq.track_levels),
            (f.track for f in seq.audio_crossfades),
        ), default=0)
        from py_premiere.models.sequence_builder import FORMATS
        base = FORMATS[preset]
        if needed > base.audio_tracks:
            if base.mono_discrete:
                raise UnverifiedShape("expanding a mono-discrete output bus is not qualified")
            preset = f"anynle-tracks/{needed}/{preset}"
            FORMATS[preset] = base._replace(audio_tracks=needed)
            report.notes.append(f"sequence {seq.name!r}: expanded audio track capacity to {needed}; output bus unchanged")
            report.unverified.append(f"sequence {seq.name!r}: expanded audio-track count requires fresh native Premiere read-back; structural checks do not qualify playback")
        sequence = project.add_sequence(seq.name, preset)
        if destination_layout is not None:
            sequence.project_item.move_to(root)
        extra_markers: list = []  # (marker, sequence ticks, note) placed on the sequence
        if seq.start:
            sequence.zero_point = Time(seq.start)
        vtracks, atracks = sequence.video_tracks, sequence.audio_tracks
        master_gain: dict[int, float] = {}
        edit_pan: dict[int, tuple] = {}
        default_panned = [0]
        for i, e in enumerate(seq.events):
            kind, payload = placement[e.clip]
            own = [m for m in seq.markers if m.owner == "edit" and m.event == i and not m.inherited]
            if own:
                payload = edit_copy(seq, i, e, kind, payload, own, extra_markers)
            start = Time(e.record_start)
            if e.video_track is not None and e.video_track > len(vtracks):
                raise UnverifiedShape(f"sequence {seq.name!r} event {i}: video track {e.video_track} > {len(vtracks)} tracks")
            if any(t > len(atracks) for t in e.audio_tracks):
                raise UnverifiedShape(f"sequence {seq.name!r} event {i}: audio track beyond {len(atracks)} tracks")
            placed = []
            if kind == "native":
                clip = model.clips[e.clip]
                gs = native_av.groups(clip)
                if e.video_track is not None:
                    placed.append(vtracks[e.video_track - 1].add_clip(payload,start))
                for j,(file,track) in enumerate(zip(event_audio_files(clip,e),e.audio_tracks)):
                    at = next(i for i,a in enumerate(clip.audio) if a is file)
                    group = gs.index((at,))
                    item = native_av.place_audio(project,atracks[track-1],payload,group,start)
                    own = e.audio_gains[j] if e.audio_gains else None
                    master_gain[id(item)] = file.gain_db if own is None else own
                    edit_pan[id(item)] = (e.audio_pans[j] if e.audio_pans else None,track)
                    placed.append(item)
                for track,channels,gain,pans in e.stereo:
                    wanted = tuple(next(i for i,a in enumerate(clip.audio) if a.avid_track == t) for t in channels)
                    item = native_av.place_audio(project,atracks[track-1],payload,gs.index(wanted),start)
                    master_gain[id(item)] = clip.audio[wanted[0]].gain_db if gain is None else gain
                    placed.append(item)
            elif kind == "grouped":
                # A multicam clip: video from the multicam sequence; the event's n-th audio
                # track plays the clip's n-th model audio file, i.e. its channel in the
                # combined (A01-first) order.
                if e.video_track is not None:
                    placed.append(vtracks[e.video_track - 1].add_clip(payload, start))
                clip = model.clips[e.clip]
                combined = list(audio_for_combined_clip(clip))
                groups = getattr(payload, "_channel_groups", [(i,) for i in range(len(combined))])
                channels = [next(i for i, c in enumerate(combined) if c is a) for a in event_audio_files(clip, e)]
                files = event_audio_files(clip, e)
                for j, (channel, track) in enumerate(zip(channels, e.audio_tracks)):
                    item = atracks[track - 1].add_clip(payload, start)
                    narrow_to_group(project, item, groups, group_for_channel(groups, channel))
                    # The multicam's inner clips already carry the master gain; an edit gain
                    # replaces it, so only the difference goes on the edit.
                    own = e.audio_gains[j] if e.audio_gains else None
                    master_gain[id(item)] = 0.0 if own is None else own - files[j].gain_db
                    edit_pan[id(item)] = (e.audio_pans[j] if e.audio_pans else None, track)
                    placed.append(item)
                # An Avid stereo edit: one item on its track playing the clip's stereo group.
                by_track = {f.avid_track: next(i for i, c in enumerate(combined) if c is f) for f in combined}
                for track, avid_channels, stereo_gain, stereo_pans in e.stereo:
                    if track > len(atracks):
                        raise UnverifiedShape(f"sequence {seq.name!r} event {i}: audio track beyond {len(atracks)} tracks")
                    _report_stereo_pans(report, seq, i, track, stereo_pans)
                    wanted = tuple(by_track[c] for c in avid_channels)
                    group = next((g for g, chans in enumerate(groups) if chans == wanted), None)
                    if group is None:
                        report.unverified.append(f"sequence {seq.name!r} event {i}: stereo edit whose clip is not grouped as that pair; not placed")
                        continue
                    item = atracks[track - 1].add_clip(payload, start)
                    narrow_to_group(project, item, groups, group)
                    left = next(f for f in clip.audio if f.avid_track == avid_channels[0])
                    master_gain[id(item)] = 0.0 if stereo_gain is None else stereo_gain - left.gain_db
                    placed.append(item)
            else:
                video_item, audio_items, stereo_builds = payload
                clip = model.clips[e.clip]
                # Items are in model order (separate) or combined A01-first order (merged).
                files = list(audio_for_combined_clip(clip)) if kind == "merged" else list(clip.audio)
                by_file = dict(zip(map(id, files), audio_items))
                event_files = event_audio_files(clip, e)
                audio_items = [by_file[id(a)] for a in event_files]
                if e.video_track is not None:
                    placed.append(vtracks[e.video_track - 1].add_clip(video_item, start))
                for j, (audio_item, track, f) in enumerate(zip(audio_items, e.audio_tracks, event_files)):
                    item = atracks[track - 1].add_clip(audio_item, start)
                    # An edit's own Avid gain is its effective gain (Avid copies the master's at
                    # cut-in); without one, the master clip's gain applies, as Premiere copies it.
                    own = e.audio_gains[j] if e.audio_gains else None
                    master_gain[id(item)] = f.gain_db if own is None else own
                    edit_pan[id(item)] = (e.audio_pans[j] if e.audio_pans else None, track)
                    placed.append(item)
                # An Avid stereo edit: one stereo clip on its track, played from the pair's build.
                for track, avid_channels, stereo_gain, stereo_pans in e.stereo:
                    _report_stereo_pans(report, seq, i, track, stereo_pans)
                    build = stereo_builds.get(tuple(avid_channels))
                    if build is None:
                        report.unverified.append(f"sequence {seq.name!r} event {i}: stereo edit for a pair the clip "
                                                 f"has no stereo build for ({kind} mode); not placed")
                        continue
                    if track > len(atracks):
                        raise UnverifiedShape(f"sequence {seq.name!r} event {i}: audio track beyond {len(atracks)} tracks")
                    item = atracks[track - 1].add_clip(build, start)
                    if getattr(build, "_stereo_nest", False):
                        declare_stereo_chain(project, item)
                    else:
                        narrow_to_group(project, item, build._channel_groups, 0)
                    # The build's inner clips carry each part's master gain (User-confirmed -6 dB
                    # inside the stereo build); an edit gain replaces it, so add only the difference.
                    left = next(f for f in clip.audio if f.avid_track == avid_channels[0])
                    master_gain[id(item)] = 0.0 if stereo_gain is None else stereo_gain - left.gain_db
                    placed.append(item)
            rect = f"0,0,{model.clips[e.clip].video.width},{model.clips[e.clip].video.height}" if kind == "grouped" and model.clips[e.clip].video else None
            for t in placed:
                span = e.duration
                if e.speed_ratio is not None:
                    set_playback_speed(t, e.speed_ratio)
                    span = e.source_duration
                # A remapped clip's In/Out are positions on its remap curve, not in the source
                # (User read-backs 2026-10-20). The curve starts one frame early: at curve time 0
                # Premiere shows the media's first frame whatever the keys say.
                start = ticks_per_unit(seq.rate) if e.time_remap is not None else e.source_in
                t.in_point = Time(start)
                t.out_point = Time(start + span)
                t.end = Time(e.record_start + e.duration)
                normalize_placement(t, rect if (rect and t._element.tag == "VideoClipTrackItem") else None)
            if e.time_remap is not None:
                clip_rate = model.clips[e.clip].video.rate
                frames = e.duration // ticks_per_unit(seq.rate)
                if len(e.time_remap) != frames:
                    raise UnverifiedShape(f"sequence {seq.name!r} event {i}: time_remap has {len(e.time_remap)} frames, the edit {frames}")
                for t in placed:
                    if t._element.tag == "VideoClipTrackItem":
                        # Values are media time, a quarter frame into each source frame: exact frame
                        # starts (49/24 s) land a hair below the frame in binary.
                        keys = [(Time((k + 1) * ticks_per_unit(seq.rate)), float((f + Fraction(1, 4)) / clip_rate)) for k, f in enumerate(e.time_remap)]
                        param = t.set_time_remap(keys)
                        # The curve's start value is what Premiere shows on the clip's first frame
                        # (py-premiere writes 0: frame 0 showed there, User export 2026-10-20).
                        start = param._element.find("StartKeyframe")
                        start.text = f"-91445760000000000,{keys[0][1]:.24f},0,0,0,0,0,0"
                report.notes.append(f"sequence {seq.name!r} event {i}: Avid Timewarp as Time Remapping, one key per frame "
                                    "from Avid's own sampled positions (provisional)")
            if e.matte is not None:
                if e.matte.inverted and len(e.matte.shapes) > 1:
                    # Inverting each Premiere mask keeps the outside of EACH shape, not the outside
                    # of them all as Avid's Key Out does.
                    report.notes.append(f"sequence {seq.name!r} event {i}: Avid AniMatte Key Out with several shapes not converted yet")
                else:
                    unit = ticks_per_unit(model.clips[e.clip].video.rate)
                    for t in placed:
                        if t._element.tag == "VideoClipTrackItem":
                            for n in apply_matte(project, t, e.matte, e.source_in, unit):
                                report.notes.append(f"sequence {seq.name!r} event {i}: {n}")
                    report.notes.append(f"sequence {seq.name!r} event {i}: Avid AniMatte as Opacity mask(s), "
                                        "per-frame path keys for tracking (provisional)")
                if any(f != (0.0, 0.0) for f in e.matte.feathers):
                    report.notes.append(f"sequence {seq.name!r} event {i}: AniMatte feather not converted yet (units not calibrated)")
                for n in e.matte.not_converted:
                    report.notes.append(f"sequence {seq.name!r} event {i}: AniMatte {n} not converted")
            if e.transform is not None:
                for t in placed:
                    if t._element.tag == "VideoClipTrackItem":
                        for n in apply_motion(project, t, e.transform):
                            report.notes.append(f"sequence {seq.name!r} event {i}: {n}")
                if e.transform.not_converted:
                    report.notes.append(f"sequence {seq.name!r} event {i}: Avid picture effect not converted yet: {', '.join(e.transform.not_converted)}")
            if e.color_correction is not None:
                cc = e.color_correction
                curves_ui8 = {k: [(x * 255, y * 255) for x, y in v] for k, v in cc.curves}
                if not avid_color.is_identity(dict(cc.tone), cc.saturation, curves_ui8):
                    # The whole modelled Avid correction as one LUT embedded in Lumetri (PRPROJ-0027),
                    # also saved as a .cube next to the project.
                    points = avid_color.cube_lut(avid_color.rgb_transform(dict(cc.tone), cc.saturation, curves_ui8), LUT_SIZE)
                    lut_dir = out_path.parent / f"{out_path.stem} LUTs"
                    try:
                        lut_dir.mkdir()
                    except FileExistsError:
                        if lut_dir.is_symlink():
                            raise FileExistsError(f"{lut_dir} exists; LUT output folder must be a real directory") from None
                        if not lut_dir.is_dir():
                            raise
                    safe = "".join(ch if ch.isalnum() or ch in " -_" else "_" for ch in seq.name)[:60]
                    requested_cube = lut_dir / f"{safe} {_timecode(e.record_start, seq.rate, seq.start).replace(':', '-')} avid cc.cube"
                    cube = write_cube(requested_cube, points, LUT_SIZE, f"Avid Color Correction, {seq.name} {_timecode(e.record_start, seq.rate, seq.start)}")
                    for t in placed:
                        if t._element.tag == "VideoClipTrackItem":
                            apply_lumetri_lut(project, t, points, LUT_SIZE, cube)
                    if cube != requested_cube:
                        report.notes.append(f"sequence {seq.name!r} event {i}: LUT export filename already used; saved as {cube.name}; existing files kept")
                    report.notes.append(f"sequence {seq.name!r} event {i}: Avid color correction as an embedded Lumetri LUT "
                                        f"({cube.name}); modelled from one measured Avid fixture, provisional")
                if cc.not_converted:
                    report.notes.append(f"sequence {seq.name!r} event {i}: Avid color correction not converted yet: {', '.join(cc.not_converted)}")
            if len(placed) > 1:
                sequence._link(placed)
            audio_placed = [t for t in placed if t._element.tag == "AudioClipTrackItem"]
            gain_db, volume, shift = split_overflow(e.audio)
            if shift:
                report.notes.append(f"sequence {seq.name!r} event {i}: volume above Premiere's +6 dB clip maximum; "
                                    f"{shift:.3f} dB moved into audio gain (same loudness)")
            for t in audio_placed:
                # A new placement carries its master clip's gain (Premiere copies it, PRPROJ-0020)
                # on top of the edit's own gain.
                total = gain_db + master_gain.get(id(t), 0.0)
                set_clip_gain(t, total)
                if e.audio != type(e.audio)():
                    set_clip_volume(project, t, volume)
                pan, track = edit_pan.get(id(t), (None, None))
                if pan is None and track is not None and model.options.match_avid_default_pan:
                    pan = -100.0 if track % 2 else 100.0  # Avid default: odd tracks left, even right
                elif pan is None and track is not None:
                    default_panned[0] += 1
                if pan is not None and pan != 0.0:
                    set_clip_pan(project, t, pan)
        if default_panned[0]:
            report.notes.append(f"sequence {seq.name!r}: {default_panned[0]} mono edit(s) had no explicit Avid pan and "
                                f"were centred (Avid would play them on its default odd-left/even-right panning; "
                                f"option: match Avid's default panning)")
        for level in seq.track_levels:
            if level.track > len(atracks):
                raise UnverifiedShape(f"sequence {seq.name!r}: track level for missing audio track {level.track}")
            set_track_level(project, atracks[level.track - 1], level.db)
        for fade in seq.audio_crossfades:
            if fade.track > len(atracks):
                raise UnverifiedShape(f"sequence {seq.name!r}: crossfade on missing audio track {fade.track}")
            try:
                add_crossfade(atracks[fade.track - 1], fade, fade.style)
            except ValueError as error:
                raise UnverifiedShape(f"sequence {seq.name!r}: {error}") from None
        unit = ticks_per_unit(seq.rate)
        filler_video: dict[int, list] = {}
        for m in seq.markers:
            if m.owner == "sequence":
                mk.add_sequence_marker(sequence, m, m.start, options, report, f"on {m.track}" if m.track else "")
            elif m.owner == "filler" and m.track and m.track.startswith("V"):
                filler_video.setdefault(int(m.track[1:]), []).append(m)
            elif m.owner == "filler":
                # Adjustment layers are video-only.
                mk.add_sequence_marker(sequence, m, m.start, options, report, f"on empty {m.track}")
        for m, start, note in extra_markers:
            mk.add_sequence_marker(sequence, m, start, options, report, note)
        for track_no, ms in sorted(filler_video.items()):
            # Markers on empty video track space ride one-frame (or marker-length) adjustment
            # layers on that track; the layer's own time equals sequence time.
            if track_no > len(vtracks):
                raise UnverifiedShape(f"sequence {seq.name!r}: filler marker on missing video track {track_no}")
            length = max(m.start + max(m.duration, unit) for m in ms)
            layer = mk.create_adjustment_layer(project, root, f"{seq.name} V{track_no} markers", unit, seq.width, seq.height,
                                               (display_format(seq.rate) if rewrapped else DISPLAY_FORMATS.get(unit, 100)), length)
            _move_to_processed_clips(project, [layer], root if destination_layout is not None else None)
            spans: dict[int, int] = {}
            for m in ms:
                mk.add_clip_marker(layer, m, m.start, options, report)
                spans[m.start] = max(spans.get(m.start, unit), m.duration or unit)
            last_end = -1
            for start in sorted(spans):
                end = start + spans[start]
                if start < last_end:
                    continue  # already covered by the previous instance
                placed = vtracks[track_no - 1].add_clip(layer, Time(start))
                placed.in_point = Time(start)
                placed.out_point = Time(end)
                placed.end = Time(end)
                normalize_placement(placed)
                last_end = end
            report.notes.append(f"sequence {seq.name!r}: {len(ms)} marker(s) on empty V{track_no} carried by an adjustment layer")
        _order_track_items(project, sequence)
    if media_relinks:
        from .rewrap import apply
        apply(project, media_relinks, narrow=not native_op1a_master)
        report.unverified.append("OP1a media binding: explicit flattened source channels; native read-back covers retained fixtures, not every profile or clip-construction mode")
    project.save(str(out_path))
    report.notes.extend(project.take_save_warnings())
    return report
