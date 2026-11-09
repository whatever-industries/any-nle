"""Explicit experimental file-backed OP1a AV masters; no hidden source sequences.

Full-source templates and logical vectors preserve explicit physical channel assignments.
Retained native checks cover bounded PCM16/24 fixtures, not every supported profile.
"""
import uuid
from pathlib import PurePosixPath, PureWindowsPath
from py_premiere.models import media_import as mi
from . import opatom as op
from .combined import _channel_groups, declare_stereo_chain
from .model import audio_for_combined_clip, event_audio_files, ticks_per_unit, TICKS_PER_SECOND


def invalid(message):
    return op.UnverifiedShape(f"native OP1a master: {message}")


def groups(clip):
    if clip.multichannel_groups:
        declared = [tuple(p) for p in clip.stereo_pairs] + [g.channels for g in clip.multichannel_groups]
        indices = {a.avid_track: i for i,a in enumerate(clip.audio)}
        answer = []
        for label in sorted(indices):
            group = next((g for g in declared if g[0] == label), None)
            if group:
                answer.append(tuple(indices[n] for n in group))
            elif not any(label in g for g in declared):
                answer.append((indices[label],))
        return answer
    order = list(audio_for_combined_clip(clip))
    indices = {id(a): i for i, a in enumerate(clip.audio)}
    return [tuple(indices[id(order[i])] for i in g) for g in _channel_groups(clip)]


def group_type(clip, group):
    if len(group) < 3:
        return len(group)-1
    labels=tuple(clip.audio[i].avid_track for i in group)
    declared=next((g for g in clip.multichannel_groups if g.channels == labels),None)
    if declared is None:
        raise invalid("missing explicit multichannel declaration")
    return 2 if declared.format == "5.1" else 3


def selected_indices(clip, group):
    if len(group)<=2:
        return group
    labels=tuple(clip.audio[i].avid_track for i in group)
    declaration=next(g for g in clip.multichannel_groups if g.channels==labels)
    indices={a.avid_track:i for i,a in enumerate(clip.audio)}
    return tuple(indices[label] for label in declaration.premiere_channels())


def adaptive_layout():
    return '[' + ','.join(['{"channellabel":0}']*32) + ']'


def sequence_channels(model,seq):
    widths={len(ch) for e in seq.events for _,ch,_,_ in e.stereo if len(ch)>2}
    if not widths:
        return 0
    if len(widths)!=1:
        raise invalid("mixed 5.1/7.1 timeline output needs explicit destination routing")
    width=next(iter(widths))
    if any(event_audio_files(model.clips[e.clip],e) or any(len(ch)!=width for _,ch,_,_ in e.stereo) for e in seq.events):
        raise invalid("mixed mono/stereo/surround timeline output needs explicit destination routing")
    return width


def preflight(model):
    used = {c for b in model.bins for c in b.clips} | {e.clip for s in model.sequences for e in s.events}
    for c in model.clips.values():
        if c.id not in used:
            continue
        v = c.video
        if v is None:
            raise invalid("requires picture-backed masters")
        from .rewrap import RATES
        if v.codec not in ('AVdn','apcs') or v.rate not in RATES:
            raise invalid("requires supported DNx/ProRes picture and an exact rewrap rate")
        if len(c.audio) > 32:
            raise invalid("more than 32 mono sources")
        labels = set()
        for a in c.audio:
            if a.bits != c.audio[0].bits:
                raise invalid('mixed PCM bit depths in one OP1a file are not supported; no sample conversion')
            if a.avid_track is None or a.avid_track <= 0 or a.avid_track in labels:
                raise invalid("missing or duplicate Avid audio labels")
            labels.add(a.avid_track)
            if a.sample_rate != 48000 or a.bits not in (16,24) or a.duration != v.duration or a.start != v.start:
                raise invalid("requires matching full-span mono 48 kHz PCM16/24 sources and clocks")
        for g in groups(c):
            if len(g) > 1 and any(c.audio[i].gain_db != c.audio[g[0]].gain_db for i in g):
                raise invalid("unequal grouped master gains require per-channel gain support")
    for s in model.sequences:
        sequence_channels(model,s)
        for e in s.events:
            c = model.clips[e.clip]
            gs = groups(c)
            if e.stereo and (e.audio.volume_db is not None or e.audio.volume_keys):
                raise invalid("stereo clip volume/automation needs a stereo volume component")
            for a in event_audio_files(c,e):
                i = next(i for i,f in enumerate(c.audio) if f is a)
                if (i,) not in gs:
                    raise invalid("mono edit of a grouped stereo member requires a derived source mapping")
            for track, channels, gain, pans in e.stereo:
                wanted = tuple(next(i for i,a in enumerate(c.audio) if a.avid_track == t) for t in channels)
                if len(wanted) not in (2,6,8) or wanted not in gs:
                    raise invalid("edit channel order must match a saved bin group")
                if any(p is not None for p in pans) and not (len(wanted)==2 and pans[0] in (None,-100.0) and pans[1] in (None,100.0)):
                    raise invalid("non-default per-channel stereo pan requires additional support")


def validate_bindings(model,mappings):
    preflight(model)
    paths = {}
    for m in mappings:
        target = m['target_path']
        if not (PurePosixPath(target).is_absolute() or PureWindowsPath(target).is_absolute()) or target == m['source_path'] or any(ord(c) < 32 or 127 <= ord(c) <= 159 or c in '\ufffe\uffff' for c in target):
            raise invalid("binding requires a distinct XML-safe absolute target path")
        if m['source_path'] in paths and paths[m['source_path']] != m:
            raise invalid("conflicting media bindings")
        paths[m['source_path']] = m
    used = {c for b in model.bins for c in b.clips} | {e.clip for s in model.sequences for e in s.events}
    wanted = {p for c in model.clips.values() if c.id in used for p in [c.video.path,*(a.path for a in c.audio)]}
    if paths.keys() - wanted:
        raise invalid("binding names an unreferenced source")
    for c in model.clips.values():
        if c.id not in used:
            continue
        vm = paths.get(c.video.path)
        if vm is None:
            raise invalid("missing picture binding")
        if vm.get('audio_stream') is not None:
            raise invalid("picture binding is an audio selector")
        for i,a in enumerate(sorted(c.audio,key=lambda a:a.avid_track)):
            am = paths.get(a.path)
            if am is None:
                raise invalid("missing audio binding")
            if am['target_path'] != vm['target_path'] or am.get('audio_stream') != i:
                raise invalid("audio binding disagrees with Avid-label-sorted output ordinals")


def add_item(project,clip,name):
    v = clip.video
    directory = str(project._path.resolve().parent)
    uid,state = op._shared_media(project,v.path,lambda uid,state:op._opatom_media(
        uid,'VideoStream',project._register(op._video_stream(v)),v.path,directory,clip,state,None,v.start))
    source = project._register(mi.new_movie_media_source(uid,v.duration))
    markers = project._register(mi.new_markers_collection(state))
    from .rewrap import video_format
    logging = op._logging(clip,ticks_per_unit(v.rate),v.duration,'2',video_format(clip),v.start)
    if clip.audio:
        logging.remove(logging.find('CaptureMode'))
    logging = project._register(logging)
    clips = [project._register(mi.new_movie_template_clip(markers,source,*op.VIDEO_LABEL))]
    sources = []
    for a in clip.audio:
        rate = TICKS_PER_SECOND // a.sample_rate
        uid,_ = op._shared_media(project,a.path,lambda uid,state:op._opatom_media(
            uid,'AudioStream',project._register(mi.new_audio_stream(rate,mi.audio_channel_layout(1),a.duration,op.SAMPLE_TYPE[a.bits])),
            a.path,directory,clip,state,rate,a.start))
        sources.append(project._register(mi.new_audio_media_source(uid,a.duration)))
    gs = groups(clip)
    order = sorted(a.avid_track for a in clip.audio)
    chains,vectors = [],[]
    if v.path in project._any_nle_mono_views:
        for i,a in enumerate(clip.audio):
            secondary=project._register(mi.new_secondary_content(sources[i],0))
            e=mi.new_audio_template_clip(markers,sources[i],[secondary],*op.VIDEO_LABEL,mi.audio_channel_layout(1))
            if a.gain_db: op._set_master_gain(e,a.gain_db)
            clips.append(project._register(e));chains.append(project._register(mi.new_master_audio_chain(1,0)))
        for g in gs:
            selected=selected_indices(clip,g)
            channels=[project._register(mi.new_channel_serializer(i,0)) for i in selected]
            vectors.append(project._register(mi.new_channel_vector(channels,group_type(clip,g))))
    else:
        for group,g in enumerate(gs):
            channel_type = group_type(clip,g)
            secondary,channels = [],[]
            for channel in range(len(clip.audio)):
                secondary.append(project._register(mi.new_secondary_content(sources[g[0]],channel)))
            selected = selected_indices(clip,g)
            for i in selected:
                channels.append(project._register(mi.new_channel_serializer(group,order.index(clip.audio[i].avid_track))))
            layout = '[' + ','.join(['{"channellabel":0}'] * len(clip.audio)) + ']'
            e = mi.new_audio_template_clip(markers,sources[g[0]],secondary,*op.VIDEO_LABEL,layout)
            if clip.audio[g[0]].gain_db:
                op._set_master_gain(e,clip.audio[g[0]].gain_db)
            clips.append(project._register(e))
            vectors.append(project._register(mi.new_channel_vector(channels,channel_type)))
            chain = mi.new_master_audio_chain(len(g),channel_type)
            if channel_type == 3:
                chain.find('AudioChannelLayout').text=adaptive_layout()
            chains.append(project._register(chain))
    group_id = project._register(mi.new_audio_channel_groups(vectors) if vectors else mi.new_channel_groups())
    master_uid = str(uuid.uuid4())
    e = mi.new_audio_master_clip(master_uid,logging,chains,clips,group_id,name) if chains else mi.new_master_clip(master_uid,logging,clips[0],group_id,name)
    master = project._register_uid(e,master_uid)
    item_uid = str(uuid.uuid4())
    e = project._register_uid(mi.new_clip_item(item_uid,master_uid,name,op.VIDEO_LABEL[0]),item_uid)
    item = project._attach_imported_item(e,item_uid,master,clips,v.path,v.duration)
    item._channel_groups = gs
    return item


def place_audio(project,track,item,group,start):
    document = project._document
    refs = [r for r in item._master_element.findall('Clips/Clip') if document.resolve(r).tag == 'AudioClip']
    original = refs[0].get('ObjectRef')
    refs[0].set('ObjectRef',refs[group].get('ObjectRef'))
    try:
        placed = track.add_clip(item,start)
    finally:
        refs[0].set('ObjectRef',original)
    groups = document.resolve(item._master_element.find('AudioClipChannelGroups'))
    vector = document.resolve(groups.findall('ClipChannelVectors/ClipChannelVectorItem')[group])
    members=[document.resolve(r) for r in vector.findall('ClipChannels/ClipChannelItem')]
    indices=[int(r.findtext('mSourceChannelIndex')) for r in members]
    source_clips=[int(r.findtext('SourceClipIndex')) for r in members]
    audio_refs=[r for r in item._master_element.findall('Clips/Clip') if document.resolve(r).tag=='AudioClip']
    mono_views=len(audio_refs)>1 and document.resolve(audio_refs[0]).findtext('AudioChannelLayout')==mi.audio_channel_layout(1)
    contents = placed._clip_element.find('SecondaryContents')
    refs = list(contents)
    for r in refs:
        contents.remove(r)
    for channel,index in enumerate(indices):
        if mono_views:
            import xml.etree.ElementTree as ET
            template=document.resolve(audio_refs[source_clips[channel]])
            source=template.find('Clip/Source').get('ObjectRef')
            if channel==0: placed._clip_element.find('Clip/Source').set('ObjectRef',source)
            key=project._register(mi.new_secondary_content(source,index))
            ref=ET.Element('SecondaryContentItem',{'ObjectRef':key})
        else:
            ref=refs[index]
        ref.set('Index',str(channel))
        contents.append(ref)
    placed._clip_element.find('AudioChannelLayout').text = adaptive_layout() if len(indices)==8 else mi.audio_channel_layout(len(indices))
    if len(indices)==8:
        import xml.etree.ElementTree as ET
        for channel in range(8,32):
            node=ET.Element('SecondaryContent',{'ClassID':'f9d004b5-cb04-4e2f-af6f-64fadc2c4be9','Version':'1'})
            ET.SubElement(node,'ChannelIndex').text='18446744073709551615'
            key=project._register(node)
            ET.SubElement(contents,'SecondaryContentItem',{'Index':str(channel),'ObjectRef':key})
    if group:
        placed._subclip_element.find('OrigChGrp').text = str(group)
    if len(item._channel_groups[group]) > 1:
        declare_stereo_chain(project,placed)
        width=len(item._channel_groups[group])
        if width>2:
            chain=document.resolve(placed._element.find('ClipTrackItem/ComponentOwner/Components'))
            chain.find('ChannelType').text='2' if width==6 else '3'
            chain.find('AudioChannelLayout').text=adaptive_layout() if width==8 else mi.audio_channel_layout(width)
    gain = placed._clip_element.find('Gain')
    if gain is not None:
        placed._clip_element.remove(gain)
    return placed
