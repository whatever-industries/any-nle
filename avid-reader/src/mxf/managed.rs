//! Physical metadata for the retained managed DNxHD Direct Out profile.
use super::*;
use std::collections::{HashMap, HashSet};

const OP1A: [u8; 16] = [
    0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x01, 0x0d, 0x01, 0x02, 0x01, 0x01, 0x01, 0x09, 0x00,
];
const DNXHD_LB: [u8; 16] = [
    0x06, 0x0e, 0x2b, 0x34, 0x04, 0x01, 0x01, 0x0a, 0x04, 0x01, 0x02, 0x02, 0x71, 0x13, 0x00, 0x00,
];
const PICTURE_KIND: [u8; 16] = [6, 14, 43, 52, 4, 1, 1, 1, 1, 3, 2, 2, 1, 0, 0, 0];
const SOUND_KIND: [u8; 16] = [6, 14, 43, 52, 4, 1, 1, 1, 1, 3, 2, 2, 2, 0, 0, 0];

#[derive(Debug)]
pub struct ManagedDirectOut {
    pub material_package: String,
    pub file_package: String,
    pub clock: QualifiedClock,
    pub picture_frames: u32,
    pub tracks: Vec<ManagedEssenceTrack>,
}

#[derive(Debug)]
pub struct ManagedEssenceTrack {
    pub track_id: u32,
    pub physical_track_number: u32,
    pub upstream_track_number: u32,
    pub sound: bool,
}

impl ManagedEssenceTrack {
    /// Resolve a decoder packet position to the complete saved essence key.
    pub fn matches_packet_key(&self, key: [u8; 16]) -> bool {
        key[..12] == [6, 14, 43, 52, 1, 2, 1, 1, 13, 1, 3, 1]
            && key[12..] == self.physical_track_number.to_be_bytes()
    }
}

/// A separate bounded check for referenced, managed OP1a sources.
pub fn read_managed_direct_out(path: &Path) -> Result<ManagedDirectOut> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|file| file.take(MAX_HEADER_BYTES).read_to_end(&mut data))
        .map_err(|error| Error::new(format!("{}: {error}", path.display()), 0))?;
    let mut managed = None;
    parse_header(&data, Some(&mut managed))?;
    managed.ok_or_else(|| Error::new("missing managed OP1a header facts", 0))
}

type Sets = HashMap<Vec<u8>, (u8, usize, usize)>;

pub(super) fn inspect(
    data: &[u8],
    sets: &Sets,
    start: usize,
    end: usize,
    id: &MxfIdentity,
    ambiguous: bool,
) -> Result<ManagedDirectOut> {
    let invalid = |reason| Error::new(reason, start);
    if ambiguous || data[start..end].get(64..80) != Some(OP1A.as_slice()) {
        return Err(invalid("requires a unique managed OP1a header"));
    }
    let [file_package] = id.file_packages.as_slice() else {
        return Err(invalid("requires one OP1a file package"));
    };
    let [material_package] = id.material_packages.as_slice() else {
        return Err(invalid("requires one OP1a material package"));
    };
    let clock = id
        .qualified_file_clock()
        .filter(|clock| {
            clock.rate == (24, 1)
                && clock.rounded_base == 24
                && !clock.drop
                && clock.source_offset == 0
                && clock.source_rate == (24, 1)
                && clock.effective_count < 24 * 60 * 60 * 24
                && id.qualified_file_clocks.iter().all(|c| c == clock)
        })
        .ok_or_else(|| invalid("managed OP1a requires a shared exact 24 fps physical clock"))?;
    let file = package(data, sets, SOURCE_PACKAGE, file_package)?;
    let material = package(data, sets, MATERIAL_PACKAGE, material_package)?;
    let upstream = package(data, sets, SOURCE_PACKAGE, &clock.source_package)?;
    let mut upstream_tracks = HashMap::new();
    for track in references(field(upstream, TAG_PACKAGE_TRACKS)?)? {
        let track = resolve(data, sets, track, TRACK)?;
        let sequence = resolve(data, sets, field(track, TAG_TRACK_SEQUENCE)?, SEQUENCE)?;
        let track_id = u32::from_be_bytes(fixed(track, 0x4801)?);
        let number = u32::from_be_bytes(fixed(track, 0x4804)?);
        if upstream_tracks
            .insert(track_id, (number, fixed::<16>(sequence, 0x0201)?))
            .is_some()
        {
            return Err(invalid("managed OP1a upstream track ID is ambiguous"));
        }
    }
    let descriptors = resolve(data, sets, field(file, 0x4701)?, 0x44)?;
    let descriptors = references(field(descriptors, 0x3f01)?)?;
    if descriptors.len() != 3 || id.file_essence_tracks != 3 {
        return Err(invalid(
            "managed OP1a requires one picture and two PCM descriptors",
        ));
    }
    let mut tracks = Vec::with_capacity(3);
    let mut track_ids = HashSet::new();
    let mut track_numbers = HashSet::new();
    let mut upstream_ids = HashSet::new();
    let mut duration = None;
    for track in references(field(file, TAG_PACKAGE_TRACKS)?)? {
        let track = resolve(data, sets, track, TRACK)?;
        let number = u32::from_be_bytes(fixed(track, 0x4804)?);
        let (span, clip) = track_components(data, sets, track)?;
        let Some(clip) = clip else {
            if number != 0 {
                return Err(invalid("managed OP1a metadata track has an essence number"));
            }
            continue;
        };
        let track_id = u32::from_be_bytes(fixed(track, 0x4801)?);
        if number == 0
            || track_id == 0
            || !track_ids.insert(track_id)
            || !track_numbers.insert(number)
            || hex32(field(clip, TAG_SOURCE_PACKAGE)?) != clock.source_package
        {
            return Err(invalid(
                "managed OP1a file track identity or physical source is inconsistent",
            ));
        }
        if duration
            .replace(span)
            .is_some_and(|previous| previous != span)
        {
            return Err(invalid("managed OP1a file track extents disagree"));
        }
        let mut matching = Vec::new();
        for reference in &descriptors {
            let (kind, descriptor) = lookup(data, sets, reference)?;
            if u32::from_be_bytes(fixed(descriptor, 0x3006)?) == track_id {
                matching.push((kind, descriptor));
            }
        }
        let [(kind, descriptor)] = matching.as_slice() else {
            return Err(invalid(
                "managed OP1a descriptor link is missing or ambiguous",
            ));
        };
        let sound = match *kind {
            CDCI_DESCRIPTOR
                if fixed::<16>(*descriptor, TAG_PICTURE_ESSENCE_CODING)? == DNXHD_LB
                    && u32::from_be_bytes(fixed(*descriptor, 0x3203)?) == 1920
                    && u32::from_be_bytes(fixed(*descriptor, 0x3202)?) == 1080
                    && fixed::<1>(*descriptor, 0x320c)? == [0]
                    && u32::from_be_bytes(fixed(*descriptor, 0x3301)?) == 8
                    && fixed::<8>(*descriptor, 0x3001)? == [0, 0, 0, 24, 0, 0, 0, 1]
                    && number.to_be_bytes()[0] == 0x15 =>
            {
                false
            }
            0x48 if fixed::<8>(*descriptor, 0x3001)? == [0, 0, 0xbb, 0x80, 0, 0, 0, 1]
                && fixed::<8>(*descriptor, 0x3d03)? == [0, 0, 0xbb, 0x80, 0, 0, 0, 1]
                && u32::from_be_bytes(fixed(*descriptor, 0x3d07)?) == 1
                && u32::from_be_bytes(fixed(*descriptor, 0x3d01)?) == 24
                && number.to_be_bytes()[0] == 0x16 =>
            {
                true
            }
            _ => {
                return Err(invalid(
                    "managed OP1a descriptor is outside the DNxHD and PCM24 profile",
                ))
            }
        };
        if i64::from_be_bytes(fixed(*descriptor, 0x3002)?) != i64::from(span) {
            return Err(invalid(
                "managed OP1a container and track frame counts disagree",
            ));
        }
        let upstream_track_id = u32::from_be_bytes(fixed(clip, 0x1102)?);
        let &(upstream_number, upstream_kind) = upstream_tracks
            .get(&upstream_track_id)
            .ok_or_else(|| invalid("managed OP1a physical source track is absent"))?;
        let expected_kind = if sound { SOUND_KIND } else { PICTURE_KIND };
        if !upstream_ids.insert(upstream_track_id)
            || upstream_kind != expected_kind
            || upstream_number == 0
            || fixed::<16>(clip, 0x0201)? != expected_kind
        {
            return Err(invalid("managed OP1a physical source track kind disagrees"));
        }
        tracks.push(ManagedEssenceTrack {
            track_id,
            physical_track_number: number,
            upstream_track_number: upstream_number,
            sound,
        });
    }
    let picture_frames = duration.ok_or_else(|| invalid("absent managed OP1a extent"))?;
    let mut referenced = HashSet::new();
    for track in references(field(material, TAG_PACKAGE_TRACKS)?)? {
        let track = resolve(data, sets, track, TRACK)?;
        let (span, Some(clip)) = track_components(data, sets, track)? else {
            continue;
        };
        let target = u32::from_be_bytes(fixed(clip, 0x1102)?);
        let expected_kind = tracks
            .iter()
            .find(|candidate| candidate.track_id == target)
            .map(|candidate| {
                if candidate.sound {
                    SOUND_KIND
                } else {
                    PICTURE_KIND
                }
            });
        if span != picture_frames
            || hex32(field(clip, TAG_SOURCE_PACKAGE)?) != *file_package
            || expected_kind != Some(fixed::<16>(clip, 0x0201)?)
            || !referenced.insert(target)
        {
            return Err(invalid(
                "managed OP1a material track does not resolve uniquely to its file track",
            ));
        }
    }
    if tracks.len() != 3
        || tracks.iter().filter(|track| track.sound).count() != 2
        || referenced != track_ids
    {
        return Err(invalid(
            "managed OP1a does not have three complete material-to-file routes",
        ));
    }
    Ok(ManagedDirectOut {
        material_package: material_package.clone(),
        file_package: file_package.clone(),
        clock,
        picture_frames,
        tracks,
    })
}

fn package<'a>(data: &'a [u8], sets: &Sets, kind: u8, uid: &str) -> Result<(&'a [u8], usize)> {
    let mut result = None;
    for &(class, start, end) in sets.values() {
        if class == kind && hex32(field((&data[start..end], start), TAG_PACKAGE_UID)?) == uid {
            if result.replace((&data[start..end], start)).is_some() {
                return Err(Error::new("ambiguous managed OP1a package", start));
            }
        }
    }
    result.ok_or_else(|| Error::new("missing managed OP1a package", 0))
}

fn lookup<'a>(data: &'a [u8], sets: &Sets, reference: &[u8]) -> Result<(u8, (&'a [u8], usize))> {
    let &(kind, start, end) = sets
        .get(reference)
        .ok_or_else(|| Error::new("unresolved managed OP1a strong reference", 0))?;
    Ok((kind, (&data[start..end], start)))
}

fn resolve<'a>(
    data: &'a [u8],
    sets: &Sets,
    reference: &[u8],
    kind: u8,
) -> Result<(&'a [u8], usize)> {
    let (actual, value) = lookup(data, sets, reference)?;
    if actual != kind {
        return Err(Error::new("unexpected managed OP1a set class", value.1));
    }
    Ok(value)
}

fn field((mut bytes, offset): (&[u8], usize), tag: u16) -> Result<&[u8]> {
    let mut found = None;
    while !bytes.is_empty() {
        let Some(header) = bytes.get(..4) else {
            return Err(Error::new("short managed OP1a local item", offset));
        };
        let key = u16::from_be_bytes(header[..2].try_into().unwrap());
        let len = usize::from(u16::from_be_bytes(header[2..].try_into().unwrap()));
        let value = bytes
            .get(4..4 + len)
            .ok_or_else(|| Error::new("short managed OP1a local value", offset))?;
        if key == tag && found.replace(value).is_some() {
            return Err(Error::new("duplicate managed OP1a local item", offset));
        }
        bytes = &bytes[4 + len..];
    }
    found.ok_or_else(|| Error::new(format!("missing managed OP1a local item {tag:04x}"), offset))
}

fn fixed<const N: usize>(set: (&[u8], usize), tag: u16) -> Result<[u8; N]> {
    field(set, tag)?
        .try_into()
        .map_err(|_| Error::new("invalid managed OP1a field width", set.1))
}

fn track_components<'a>(
    data: &'a [u8],
    sets: &Sets,
    track: (&[u8], usize),
) -> Result<(u32, Option<(&'a [u8], usize)>)> {
    if fixed::<8>(track, TAG_TRACK_EDIT_RATE)? != [0, 0, 0, 24, 0, 0, 0, 1]
        || !zero_track_origin(track.0, track.1)?
    {
        return Err(Error::new(
            "managed OP1a track requires zero origin and exact 24 fps",
            track.1,
        ));
    }
    let sequence = resolve(data, sets, field(track, TAG_TRACK_SEQUENCE)?, SEQUENCE)?;
    let components = references(field(sequence, TAG_COMPONENTS)?)?;
    let [reference] = components.as_slice() else {
        return Err(Error::new(
            "managed OP1a track requires a single component",
            sequence.1,
        ));
    };
    let (kind, component) = lookup(data, sets, reference)?;
    if kind == TIMECODE_COMPONENT {
        return Ok((0, None));
    }
    let count = i64::from_be_bytes(fixed(sequence, TAG_DURATION)?);
    if kind != SOURCE_CLIP
        || fixed::<8>(component, TAG_SOURCE_START)? != [0; 8]
        || i64::from_be_bytes(fixed(component, TAG_DURATION)?) != count
        || !(1..=1024).contains(&count)
    {
        return Err(Error::new(
            "managed OP1a component is outside the bounded zero-offset profile",
            sequence.1,
        ));
    }
    Ok((count as u32, Some(component)))
}
