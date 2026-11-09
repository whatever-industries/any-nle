"""Explicit OP1a bindings. Uniform-depth mono audio uses flattened source channels.
No model/1 or default writer changes.
"""
from fractions import Fraction
from pathlib import Path
from py_premiere.xml.mutations import insert_leaf_before
from py_premiere.models.media_paths import relative_media_path

RATES = tuple(Fraction(n,d) for n,d in [(24,1),(24000,1001),(25,1),(30,1),(30000,1001),
    (48,1),(48000,1001),(50,1),(60,1),(60000,1001),(72,1),(72000,1001),(75,1),
    (90,1),(90000,1001),(96,1),(96000,1001),(100,1),(120,1),(120000,1001)])

def display_format(rate, drop=False):
    if rate not in RATES or (drop and rate not in [Fraction(n,1001) for n in [30000,60000,90000,120000]]):
        raise ValueError('unsupported rewrap video rate/timecode')
    return {Fraction(24):100,Fraction(24000,1001):110,Fraction(25):101,Fraction(30):104,
        Fraction(30000,1001):102 if drop else 103,Fraction(48):113,Fraction(50):105,
        Fraction(60):108,Fraction(60000,1001):106 if drop else 107}.get(rate,109)

def video_format(clip):
    return str(display_format(clip.video.rate,clip.time_display in (102,106)))

def validate_audio_formats(model, mappings):
    bound = {m['source_path']: m['target_path'] for m in mappings if m.get('audio_stream') is not None}
    formats = {}
    for c in model.clips.values():
        for a in c.audio:
            target = bound.get(a.path)
            if target is None:
                continue
            if a.sample_rate != 48000 or a.bits not in (16,24):
                raise ValueError('rewrap audio requires mono 48 kHz PCM16/24')
            format = (a.sample_rate, a.bits)
            if target in formats and formats[target] != format:
                raise ValueError('mixed PCM bit depths in one OP1a file are not supported; no sample conversion')
            formats[target] = format

def apply(project, mappings, *, narrow=True):
    by_path = {}
    channels = {}
    for m in mappings:
        index = m.get('audio_stream')
        if index is not None:
            if not isinstance(index, int) or isinstance(index, bool) or not 0 <= index < 32:
                raise ValueError('rewrap audio ordinal outside 0..31')
            channels.setdefault(m['target_path'], set()).add(index)
        if any(ord(c) < 32 or 127 <= ord(c) <= 159 or c in '\ufffe\uffff' for c in m['target_path']):
            raise ValueError('rewrap target path cannot be saved in XML')
        if not Path(m['target_path']).is_absolute() or m['source_path'] == m['target_path']:
            raise ValueError('rewrap needs a distinct absolute target path')
        if m['source_path'] in by_path and by_path[m['source_path']] != m:
            raise ValueError('conflicting rewrap mappings')
        by_path[m['source_path']] = m
    if any(indices != set(range(len(indices))) for indices in channels.values()):
        raise ValueError('rewrap audio bindings must describe every output channel')
    ids = {e.get('ObjectID'): e for e in project._document.root if e.get('ObjectID')}
    found = set()
    states = {}
    state_remap = {}
    for node in project._document.root:
        if node.tag != 'Media':
            continue
        m = by_path.get(node.findtext('FilePath'))
        if m is None:
            continue
        if (node.find('AudioStream') is not None) != (m.get('audio_stream') is not None):
            raise ValueError('rewrap media kind disagrees with project source')
        for tag in ['FilePath', 'ActualMediaFilePath', 'RelativePath']:
            child = node.find(tag)
            if child is None:
                raise ValueError(f'rewrap Media lacks {tag}')
            child.text = (relative_media_path(m['target_path'], str(project._path.parent))
                          if tag == 'RelativePath' else m['target_path'])
        key, state, modification = [node.find(tag) for tag in ['FileKey', 'ContentAndMetadataState', 'ModificationState']]
        if key is None or state is None or modification is None:
            raise ValueError('rewrap Media lacks physical file state')
        old = state.text
        shared = states.setdefault(m['target_path'], (key.text, state.text, dict(modification.attrib), modification.text))
        key.text, state.text = shared[:2]
        modification.attrib = dict(shared[2])
        modification.text = None if (m.get('audio_stream') or 0) > 0 else shared[3]
        state_remap[old] = shared[1]
        found.add(m['source_path'])
        index = m.get('audio_stream')
        if index is not None:
            child = node.find('StreamNumber')
            if child is not None:
                node.remove(child)
            mono_mov=not narrow and m['target_path'].lower().endswith('.mov') and len(channels[m['target_path']])>2
            if mono_mov and index>0:
                insert_leaf_before(node,'ContentAndMetadataState','StreamNumber',str(index))
            if narrow:
                insert_leaf_before(node, 'ContentAndMetadataState', 'SingleAudioChannel', str(index))
            stream = ids[node.find('AudioStream').get('ObjectRef')]
            stream.find('AudioChannelLayout').text = '[' + ','.join(['{"channellabel":0}'] * (1 if mono_mov else len(channels[m['target_path']])) ) + ']'
            if narrow:
                insert_leaf_before(stream, 'FrameRate', 'SingleChannel', str(index))
    if len(found) != len(by_path):
        raise ValueError('rewrap mapping names a source absent from the written project')
    for node in project._document.root.iter('LastContentState'):
        if node.text in state_remap:
            node.text = state_remap[node.text]
