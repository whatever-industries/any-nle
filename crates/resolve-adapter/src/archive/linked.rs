//! Construct a project from prepared bin, media and timeline records.
use super::*;
use crate::dissolve::{self, Dissolve};
use crate::{
    linked::{
        is_2997, mixer::DirectMixer, movie_audio_sample_extent, samples_per_frame, Media,
        MovieVideo, PreparedProject, Profile, Timeline, timing::Timing,
    },
    turnover::Templates,
};
use graph::{Rational, Value};

const TEMPLATE_23976: &str = "4b2895fa6827801bce87bff7f657dae6454f7011459b689d1983bb11a017ccfc";
const TEMPLATE_24: &str = "e2a3a1ad16be932947914d866b3c8bb49b549ffd5869a06a9fc9eb576bf21bf5";
const TEMPLATE_STEREO_WAV_24: &str = "43bfa214eac8a096c63bb8b6d8058863c6caf6b0bae03d7da5fc67208e5b910a";
const TIMELINE_STEREO_WAV_24_DONOR: &str = "SeqContainer/2be02669-f097-468a-b921-f7ece69ac022.xml";
const TEMPLATE_25: &str = "3a9721f6a6d123b28389b8d20824bfcfde2ace83534e041de80587dd3b37929c";
const TIMELINE_25_DONOR: &str = "SeqContainer/d92e8869-b608-4e44-958a-6662b238afb4.xml";
const TEMPLATE_STEREO_25: &str = "f4ab29c6baba526708930f0549065bf2dbff2778f733c4656e2464a3bc94591e";
const TIMELINE_STEREO_25_DONOR: &str = "SeqContainer/a6276b9a-c5f8-4179-bde7-8a7ae6768ec5.xml";
const TEMPLATE_STEREO_MONO_25: &str = "7e432d101ccafcf1474638f7d44a4f95cb15d457365bfe2af7377b00e2e83e94";
const TIMELINE_STEREO_MONO_25_DONOR: &str = "SeqContainer/6cfa0ade-e84e-4483-a838-a32132c2efe7.xml";
const TEMPLATE_2997_DF: &str = "8682789306a7452cfb25e6a59a8e7136a21023d89b43df1eff4c4fe9923f42f8";
const TIMELINE_2997_DF_DONOR: &str = "SeqContainer/785f172e-1544-45cc-be16-f3f2e67936e8.xml";
const TEMPLATE_UHD_25: &str = "b601597551a420a0b4469c132e32b515ad5b9c7f19889208f0c3fb28b6a619e5";
const TIMELINE_UHD_25_DONOR: &str = "SeqContainer/938667b8-5dfd-4dc0-893b-4a0c3e88211b.xml";
const TEMPLATE_UHD_23976: &str = "761f1ba75ff3435ae525d8025583c4cadca30fdc0156dcc20872b91aee066721";
const TIMELINE_UHD_23976_DONOR: &str = "SeqContainer/e877d5be-159e-4955-b0e8-7156917f19a6.xml";
const TEMPLATE_UHD_DF: &str = "db653bc2658d3474efb8b1e594ebead13506c6af688e9dde6a956be82e1557ff";
const TIMELINE_UHD_DF_DONOR: &str = "SeqContainer/1652bcc5-d67c-4348-b982-8143b1783e0b.xml";
const TWO_BUS_TEMPLATE: &str = "51e9255ae6fed4f82b7a297531a7fab0bff8353153bfe39e7f797bb7296570cf";
const TWO_BUS_FOLDER: &str = "MediaPool/Master/000_Originals/MpFolder.xml";
const THREE_MONO_TEMPLATE: &str = "346c93372828c5068323833437cbb4331ea1b8520d546e75850ea522926b771b";
const THREE_MONO_SEQUENCE: &str = "SeqContainer/2ce73793-25f7-49d0-88d6-e3c95a01841f.xml";
const NEST_TEMPLATE: &str = "48aa85bd7184d72532b52f993158e7c515b9e820887855f4e1065798b7e5263a";
const NEST_PARENT: &str = "SeqContainer/d35c263e-bf97-4a72-8876-c35e133c6601.xml";
const NEST_COMPOUND: &str = "SeqContainer/bdfee1f6-00ff-4a76-94fb-662551ca1846.xml";
const STEREO_NEST_TEMPLATE: &str = "42cd49d9f9984c0f6961edb145ba6922ae44f39565dc221b254ff7e53a27ab32";
const STEREO_NEST_PARENT: &str = "SeqContainer/30f61c21-7bda-4ed6-8d56-c5c0000c5934.xml";
const MEDIA_DONOR: &str = "MediaPool/Master/002_Linked/000_Audio/MpFolder.xml";
const TIMELINE_DONOR: &str = "SeqContainer/93a83077-933a-4ed8-9595-e12534b57e18.xml";
const PRORES_REC709: &str = "80087810027002789e01800166880132980100d80100";
const AVC_UNTAGGED: &str = "80089403100228108801329001d080a006980100d80100";
const AVC_REC709: &str = "80089403100228107002789e018001668801329001d080a006980100d80100";
const AVC_UHD_UNTAGGED: &str = "80089403100228108801329001e680a006980100d80100";
const HEVC_MAIN10_UNTAGGED: &str = "8008940310028801329001f08110980100d80100";

pub fn publish_linked(
    prepared: &PreparedProject,
    layout_template: &Path,
    profile_template: &Path,
    output: &Path,
) -> Result<Published> {
    publish_turnover(
        prepared,
        layout_template,
        profile_template,
        None,
        None,
        output,
    )
}

pub fn publish_turnover(
    prepared: &PreparedProject,
    layout_template: &Path,
    profile_template: &Path,
    atom_template: Option<&Path>,
    group_template: Option<&Path>,
    output: &Path,
) -> Result<Published> {
    publish_linked_with_templates(
        prepared,
        &Templates {
            layout: layout_template.into(),
            profile: profile_template.into(),
            atom: atom_template.map(Path::to_owned),
            group: group_template.map(Path::to_owned),
            fairlight: None,
            nested: None,
        },
        output,
    )
}

pub fn publish_linked_with_templates(
    prepared: &PreparedProject,
    templates: &Templates,
    output: &Path,
) -> Result<Published> {
    publish_turnover_checked(prepared, templates, output, || Ok(()))
}

pub(crate) fn publish_turnover_checked(
    prepared: &PreparedProject,
    templates: &Templates,
    output: &Path,
    recheck: impl FnOnce() -> Result<()>,
) -> Result<Published> {
    check_output(output)?;
    let (members, identities) = assemble_turnover(prepared, templates)?;
    write_checked_archive(members, output, |stage| {
        verify_stage(prepared, &identities, stage)?;
        for media in &prepared.media {
            media.recheck()?;
        }
        recheck()
    })
}

fn assemble_turnover(
    prepared: &PreparedProject,
    templates: &Templates,
) -> Result<(Members, Identities)> {
    let template = |role: &str, path: &Path, hash: &str, count| -> Result<Members> {
        load_skeleton(path, hash, count)
            .map_err(|error| format!("Resolve {role} template {}: {error}", path.display()).into())
    };
    let layout = template("layout", &templates.layout, TEMPLATE_23976, 13)?;
    let stereo = prepared.timelines.iter().any(|row| row.stereo);
    let (hash, count, timeline) = match (prepared.canvas, prepared.rate.num(), prepared.rate.den()) {
        ((1920, 1080), 24, 1) if stereo =>
            (TEMPLATE_STEREO_WAV_24, 4, Some(("Stereo WAV 24", TIMELINE_STEREO_WAV_24_DONOR))),
        ((1920, 1080), 24, 1) => (TEMPLATE_24, 10, None),
        ((1920, 1080), 25, 1) if prepared.timelines.iter().any(|row| row.stereo && !row.lanes[2].is_empty()) =>
            (TEMPLATE_STEREO_MONO_25, 4, Some(("Stereo A1 Mono A2", TIMELINE_STEREO_MONO_25_DONOR))),
        ((1920, 1080), 25, 1) if stereo =>
            (TEMPLATE_STEREO_25, 4, Some(("Stereo PCM 25", TIMELINE_STEREO_25_DONOR))),
        ((1920, 1080), 25, 1) => (TEMPLATE_25, 4, Some(("Linked 25", TIMELINE_25_DONOR))),
        ((1920, 1080), 30000, 1001) => (TEMPLATE_2997_DF, 4, Some(("Linked DF", TIMELINE_2997_DF_DONOR))),
        ((1920, 1080), 24000, 1001) => (TEMPLATE_23976, 13, None),
        ((3840, 2160), 24000, 1001) => (TEMPLATE_UHD_23976, 4, Some(("Linked UHD", TIMELINE_UHD_23976_DONOR))),
        ((3840, 2160), 25, 1) => (TEMPLATE_UHD_25, 4, Some(("Linked UHD", TIMELINE_UHD_25_DONOR))),
        ((3840, 2160), 30000, 1001) => (TEMPLATE_UHD_DF, 4, Some(("Linked UHD", TIMELINE_UHD_DF_DONOR))),
        _ => return Err("missing pinned template for Resolve project rate and canvas".into()),
    };
    let profile = template("profile", &templates.profile, hash, count)?;
    let atom_donor = if prepared
        .media
        .iter()
        .any(|m| matches!(&m.profile, Profile::BoundMxf(binding) if binding.atom.is_some()))
    {
        let path = templates
            .atom
            .as_deref()
            .ok_or("split MXF project requires its pinned media template")?;
        let members = template("split MXF", path, SKELETON_SHA256, 6)?;
        Some(block(member(&members, SOURCE)?, "Sm2MpVideoClip")?)
    } else {
        None
    };
    let op1a_donor = if prepared.media.iter()
        .any(|m| matches!(&m.profile, Profile::BoundMxf(binding) if binding.atom.is_none()))
    {
        Some(super::group::camera_donor(templates.group.as_deref()
            .ok_or("OP1a cameras require the pinned Resolve Group template")?)?)
    } else {
        None
    };
    let two_bus = prepared
        .timelines
        .iter()
        .any(|row| row.mixer.is_some_and(|m| m.buses == 2));
    let three_mono = prepared.timelines.iter().any(|row| !row.lanes[4].is_empty());
    if two_bus && three_mono {
        return Err("two-bus and three-mono Resolve templates cannot be combined".into());
    }
    let fairlight = if two_bus {
        let path = templates
            .fairlight
            .as_deref()
            .ok_or("missing pinned two-bus Fairlight template")?;
        let donor = template("two-bus Fairlight", path, TWO_BUS_TEMPLATE, 7)?;
        let sequence = block(member(&donor, TWO_BUS_FOLDER)?, "Sm2Sequence")?;
        Some(
            Mixer::decode(&sequence_fields(&sequence)?)?
                .ok_or("missing persisted Fairlight model in the pinned template")?,
        )
    } else {
        None
    };
    let wider_layout = if three_mono {
        let path = templates.fairlight.as_deref().ok_or("missing pinned three-mono layout template")?;
        Some(template("three-mono layout", path, THREE_MONO_TEMPLATE, 4)?)
    } else {
        None
    };
    let nest_template = if prepared.timelines.iter().any(|row| !row.nests.is_empty()) {
        let path = templates.nested.as_deref().ok_or("missing pinned Resolve nesting template")?;
        if stereo {
            Some(template("stereo nesting", path, STEREO_NEST_TEMPLATE, 5)?)
        } else {
            Some(template("nesting", path, NEST_TEMPLATE, 9)?)
        }
    } else { None };
    let (timeline_profile, name, path) = match timeline {
        Some((name, path)) => (&profile, name, path),
        None => (&layout, "Linked A", TIMELINE_DONOR),
    };
    let pool = named(member(timeline_profile, POOL)?, "Sm2MpTimelineClip", name)?;
    let donors = TimelineDonors {
        pool: &pool,
        container: member(timeline_profile, path)?,
        fairlight: fairlight.as_ref(),
    };
    build(
        prepared,
        &layout,
        &profile,
        [atom_donor.as_deref(), op1a_donor.as_deref()],
        templates.group.as_deref(),
        &donors,
        wider_layout.as_ref(),
        nest_template.as_ref(),
    )
}

pub(crate) fn review_turnover(
    prepared: &PreparedProject,
    templates: &crate::turnover::Templates,
    output: &Path,
) -> Result<()> {
    check_output(output)?;
    assemble_turnover(prepared, templates)?;
    Ok(())
}

fn member<'a>(members: &'a Members, path: &str) -> Result<&'a str> {
    members
        .get(path)
        .map(String::as_str)
        .ok_or_else(|| format!("missing template member {path}").into())
}

fn blocks(name: &str) -> Result<Regex> {
    Ok(Regex::new(&format!(
        r"(?s)<{name}(?: [^>]+)?>.*?</{name}>"
    ))?)
}

fn named(source: &str, class: &str, name: &str) -> Result<String> {
    let re = blocks(class)?;
    let label = format!("<Name>{}</Name>", xml(name));
    let mut hits = re
        .find_iter(source)
        .filter(|item| item.as_str().contains(&label));
    let found = hits
        .next()
        .ok_or_else(|| format!("missing pinned {class} {name}"))?;
    if hits.next().is_some() {
        return Err("ambiguous pinned template object name".into());
    }
    Ok(found.as_str().to_owned())
}

fn container_field(s: &mut String, name: &str, contents: &str) -> Result<()> {
    let re = Regex::new(&format!(r"(?s)<{name}(?:/>|>.*?</{name}>)"))?;
    let range = one(&re, s)?.range();
    s.replace_range(range, &format!("<{name}>{contents}</{name}>"));
    Ok(())
}

fn new_ids(xml: &str) -> Result<BTreeMap<String, String>> {
    let re = Regex::new(
        r#"DbId="([a-f0-9-]{36})"|<Unique(?:SequenceId|MediaPoolItemId)>([a-f0-9-]{36})</"#,
    )?;
    let mut ids = BTreeMap::new();
    for m in re.captures_iter(xml) {
        let id = m
            .get(1)
            .or_else(|| m.get(2))
            .ok_or("missing template object capture")?;
        Uuid::parse_str(id.as_str())?;
        if ids.insert(id.as_str().to_owned(), uid()).is_some() {
            return Err("duplicate template object identity".into());
        }
    }
    Ok(ids)
}

fn remap(mut s: String, ids: &BTreeMap<String, String>) -> String {
    for (old, new) in ids {
        s = s.replace(old, new);
    }
    s
}

fn remap_blob(mut data: Vec<u8>, ids: &BTreeMap<String, String>) -> Vec<u8> {
    for (old, new) in ids {
        let utf16 = |s: &str| {
            s.encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>()
        };
        for (from, to) in [
            (old.as_bytes().to_vec(), new.as_bytes().to_vec()),
            (utf16(old), utf16(new)),
        ] {
            let mut offset = 0;
            while let Some(index) = data[offset..].windows(from.len()).position(|x| x == from) {
                offset += index;
                data[offset..offset + to.len()].copy_from_slice(&to);
                offset += to.len();
            }
        }
    }
    data
}

fn rate_bytes(rate: Rational) -> [u8; 16] {
    let mut encoded = [0; 16];
    encoded[..8].copy_from_slice(&fps(rate).to_le_bytes());
    encoded
}

fn fps(rate: Rational) -> f64 {
    rate.num() as f64 / rate.den() as f64
}

fn prefix_name(item: &mut String, name: &str) -> Result<()> {
    let end = item.find("<PTZRPreset>").unwrap_or(item.len());
    let mut head = item[..end].to_owned();
    set(&mut head, "Name", &xml(name))?;
    item.replace_range(..end, &head);
    Ok(())
}

/// Saved QuickTime PCM caches include both an epoch and a calendar string.
/// Store the calendar string in UTC to avoid locale-dependent output.
fn resolve_file_stamp(seconds: u64) -> Result<String> {
    if seconds > 253402300799 {
        return Err("Resolve media timestamp outside the representable calendar".into());
    }
    let days = (seconds / 86400) as i64;
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let weekday = ["Thu","Fri","Sat","Sun","Mon","Tue","Wed"][days as usize % 7];
    let month = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"][(month-1) as usize];
    Ok(format!("{weekday} {month} {day:2} {:02}:{:02}:{:02} {year:04}",
        seconds / 3600 % 24, seconds / 60 % 60, seconds % 60))
}

fn clip_cache(media: &Media, codec: &[u8], video: bool) -> Result<String> {
    let folder = utf8_path(media.path.parent().ok_or("missing linked media parent")?)?;
    let file = media
        .path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("invalid linked basename")?;
    let mut data = Vec::new();
    pb_bytes(1, folder.as_bytes(), &mut data);
    pb_bytes(2, file.as_bytes(), &mut data);
    let embedded_pcm = matches!(media.profile,Profile::Mov {pcm:Some(_),..});
    let mono_atom = matches!(media.profile, Profile::MonoAtom { .. });
    let hevc = matches!(media.profile, Profile::Mov { video: MovieVideo::HevcMain10, .. });
    if embedded_pcm || mono_atom || hevc {
        let seconds = media.calendar_seconds.unwrap_or(media.modified_micros / 1_000_000);
        pb_bytes(3, resolve_file_stamp(seconds)?.as_bytes(), &mut data);
    }
    pb_bytes(5, codec, &mut data);
    if video {
        pb_bytes(6, file.as_bytes(), &mut data);
        pb_bytes(7, uid().as_bytes(), &mut data);
    }
    pb_int(13, media.modified_micros, &mut data);
    if video && hevc {
        pb_int(14, 2570, &mut data);
    } else if video && matches!(media.profile, Profile::Mov { video: MovieVideo::ProResHq, pcm: Some(_), .. }) {
        pb_int(14, 244, &mut data);
    } else if matches!(
        media.profile,
        Profile::Mov {
            quicktime_timecode: None,
            video: MovieVideo::ProResHq,
            ..
        }
    ) {
        pb_int(14, 24584, &mut data);
    }
    pb_int(15, 8, &mut data);
    pb_int(16, 100, &mut data);
    pb_int(
        18,
        if mono_atom {
            16388
        } else if video || codec == b"AAC" || embedded_pcm {
            16384
        } else {
            32768
        },
        &mut data,
    );
    pack(&data)
}

fn embedded_audio(item: &mut String, source: &str, media: &Media, frames: i64, rate: Rational, pcm: Option<i64>, stereo: bool) -> Result<()> {
    let donor = named(source, "Sm2MpAudioClip", "Signals-24.wav")?;
    let base = block(&donor, "BtAudioInfo")?;
    let id = uid();
    let samples = if matches!(&media.profile, Profile::Mov { video: MovieVideo::Avc | MovieVideo::HevcMain10, .. }) {
        movie_audio_sample_extent(frames, rate)?
    } else {
        frames * samples_per_frame(rate)?
    };
    let (codec, depth) = match pcm { Some(16) => ("Linear PCM",1), Some(24) => ("Linear PCM",2), _ => ("AAC",3) };
    let start = match &media.profile {
        Profile::Mov { avid_source_timecode: Some(start), .. } if pcm.is_some() => *start as f64 / fps(rate),
        _ => 0.0,
    };
    let track_id = uid();
    let mut track = vec![
        ("UniqueId", Text(&track_id)),
        ("StartTime", Double(start)),
        ("SampleRate", Uint(48000)),
        ("NumChannels", Int(2)),
        ("IdxTrack", Int(0)),
        ("Duration", Long(samples)),
        ("DbType", Text("BtAudioTrack")),
        ("CodecName", Text(codec)),
        ("BitDepth", Uint(depth)),
    ];
    if pcm.is_none() {
        // Preserve the established AVC/AAC field order.
        track.insert(track.len()-1, ("ChannelLayout", Int(2)));
    }
    let track = qt(&track);
    let audio = bind_audio(
        &base,
        &id,
        &clip_cache(media, codec.as_bytes(), false)?,
        &qt(&[("0", Bytes(&track))]),
    )?;
    container_field(
        item,
        "EmbeddedAudioVec",
        &format!("<Element>{audio}</Element>"),
    )?;
    let virtual_tracks = if stereo {
        String::new() // Native stereo uses the embedded track's default left/right mapping.
    } else {
        let mono_left = mono_selector(0, false)?;
        let mono_right = mono_selector(1, false)?;
        hex::encode(qt(&[("1", Bytes(&mono_right)), ("0", Bytes(&mono_left))]))
    };
    set(item, "VirtualAudioTracksBA", &virtual_tracks)?;
    let left = qt(&[("MediaRef", Text(&id))]);
    let right = qt(&[("MediaRef", Text(&id)), ("ChannelIdx", Int(1))]);
    let channels = qt(&[("1", Bytes(&right)), ("0", Bytes(&left))]);
    let range = extent(start, samples as f64 / 48000.0);
    let mut route = vec![
        ("Type", Uint(0)),
        ("MediaExtents", Bytes(&range)),
        ("CodecName", Text(codec)),
        ("ChannelVecBA", Bytes(&channels)),
    ];
    if pcm.is_some() {
        route.push(("BitDepth",Uint(depth)));
    }
    let route = qt(&route);
    let routes = qt(&[("0", Bytes(&route))]);
    replace_fields(item, |data| {
        replace_bytes(&data, 1, |a| {
            replace_bytes(a, 1, |b| {
                crate::binary::rewrite_bytes(b, 5, |_| Ok(Some(routes)))
            })
        })
    })
}

pub(super) fn mono_selector(channel: usize, atom: bool) -> Result<Vec<u8>> {
    let mut channels = [0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, 1];
    let index = u8::try_from(channel + 1)?;
    if atom {
        channels[10] = 0x40 * index;
    } else {
        channels[11] = index;
    }
    Ok(qt(&[
        ("ChannelsBA", Bytes(&channels)),
        ("AudioType", Int(1)),
    ]))
}

fn replace_fields(
    item: &mut String,
    update: impl FnOnce(Vec<u8>) -> Result<Vec<u8>>,
) -> Result<()> {
    let re = Regex::new(r"<FieldsBlob>([a-f0-9]+)</FieldsBlob>")?;
    let payload = re
        .captures(item)
        .and_then(|m| m.get(1))
        .ok_or("missing template FieldsBlob")?;
    let updated = pack(&update(unpack(payload.as_str())?)?)?;
    item.replace_range(payload.range(), &updated);
    Ok(())
}

fn pool_item(
    source: &str,
    media: &Media,
    bin: &str,
    project_rate: Rational,
    mxf_donors: [Option<&str>; 2],
    stereo: bool,
) -> Result<(String, String)> {
    if let Some(native) = &media.native {
        let ids = new_ids(&native.xml)?;
        let mut item = remap(native.xml.clone(), &ids);
        let id = dbid(&item)?.to_owned();
        replace_fields(&mut item, |data| Ok(remap_blob(data, &ids)))?;
        set(&mut item, "MpFolder", bin)?;
        prefix_name(&mut item, &media.name)?;
        return Ok((id, item));
    }
    let donor = match &media.profile {
        Profile::Mov { .. } => named(source, "Sm2MpVideoClip", "Counter-23976.mov")?,
        Profile::Wav { bits, channels, .. } => {
            let name = format!(
                "{}-{bits}.wav",
                if *channels == 1 { "Mono" } else { "Signals" }
            );
            named(source, "Sm2MpAudioClip", &name)?
        }
        Profile::MonoAtom { bits, .. } =>
            named(source, "Sm2MpAudioClip", &format!("Mono-{bits}.wav"))?,
        Profile::BoundMxf(binding) => {
            let donor = mxf_donors[usize::from(binding.atom.is_none())]
                .ok_or("missing linked MXF template")?;
            let id = uid();
            let (folder, file) = media_path_parts(&binding.path)?;
            let mut item = bind_pool_item(donor, binding, folder, file, &id)?;
            set(&mut item, "MpFolder", bin)?;
            prefix_name(&mut item, &media.name)?;
            return Ok((id, item));
        }
    };
    let ids = new_ids(&donor)?;
    let mut item = remap(donor, &ids);
    let id = dbid(&item)?.to_owned();
    set(&mut item, "MpFolder", bin)?;
    prefix_name(&mut item, &media.name)?;
    match &media.profile {
        Profile::Mov {
            rate,
            frames,
            raster,
            tagged_rec709,
            video: codec,
            aac,
            pcm,
            ..
        } => {
            let old = block(&item, "BtVideoInfo")?;
            let mut video = old.clone();
            set(
                &mut video,
                "Clip",
                &clip_cache(media, codec.cache_codec()?.as_bytes(), true)?,
            )?;
            let timing_id = uid();
            let frame_rate = rate_bytes(*rate);
            let mut timing = vec![
                ("UniqueId", Text(&timing_id)),
                ("StartFrame", Int(0)),
                ("NumFrames", Int(i32::try_from(*frames)?)),
                ("FrameRate", Bytes(&frame_rate)),
            ];
            let timecode = media.profile.movie_timecode().map(crate::linked::non_drop_24);
            if let Some(timecode) = &timecode {
                timing.push(("Timecode", Text(timecode)));
            }
            timing.push(("DbType", Text("BtVideoTime")));
            set(&mut video, "Time", &hex::encode(qt(&timing)))?;
            if *codec != MovieVideo::ProResHq {
                let (frame_size, radiometry) = match (codec, raster) {
                    (MovieVideo::Avc, (1920, 1080)) if *tagged_rec709 => (3126784, AVC_REC709),
                    (MovieVideo::Avc, (1920, 1080)) => (3126784, AVC_UNTAGGED),
                    (MovieVideo::Avc, (3840, 2160)) if !tagged_rec709 => (12457984, AVC_UHD_UNTAGGED),
                    (MovieVideo::HevcMain10, (1920, 1080)) => (6237184, HEVC_MAIN10_UNTAGGED),
                    _ => return Err("unqualified Resolve AVC or HEVC media cache geometry".into()),
                };
                let mut resolution = [0; 16];
                resolution[..8].copy_from_slice(&u64::from(raster.0).to_be_bytes());
                resolution[8..].copy_from_slice(&u64::from(raster.1).to_be_bytes());
                let geometry = qt(&[
                    ("UniqueId", Text(&uid())),
                    ("Resolution", Bytes(&resolution)),
                    ("FrameSize", Int(frame_size)),
                    ("DbType", Text("BtGeometry")),
                ]);
                set(&mut video, "Geometry", &hex::encode(geometry))?;
                set(&mut video, "Radiometry", radiometry)?;
            } else if *tagged_rec709 {
                set(&mut video, "Radiometry", PRORES_REC709)?;
            }
            let metadata = qt(&[
                ("UniqueId", Text(&uid())),
                ("DbType", Text("BtVideoMetadata")),
            ]);
            set(&mut video, "VideoMetadata", &hex::encode(metadata))?;
            replace_once(&mut item, &old, &video)?;
            if *aac || pcm.is_some() {
                embedded_audio(&mut item, source, media, *frames, *rate, *pcm, stereo)?;
            }
            if let Some(proxy) = &media.proxy {
                let proxy_cache = super::proxy_cache::build(proxy)?;
                replace_fields(&mut item, |data| crate::binary::rewrite_bytes(&data,2,|old| {
                    if old.is_some() { return Err("unexpected proxy in Resolve media skeleton".into()); }
                    Ok(Some(proxy_cache))
                }))?;
            }
        }
        Profile::Wav {
            samples,
            bits,
            channels,
        } => standalone_audio(&mut item, media, project_rate, *samples, *bits, *channels, None)?,
        Profile::MonoAtom { frames, bits, start, package, material } => {
            let identity = qt(&[
                ("tapeMobId", Bytes(&[0; 32])),
                ("sourceMobId", Bytes(package)),
                ("masterMobId", Bytes(material)),
            ]);
            set(&mut item, "VirtualAudioTracksBA", "")?;
            standalone_audio(
                &mut item, media, project_rate, frames * samples_per_frame(project_rate)?,
                *bits, 1, Some((seconds(*start), &identity)),
            )?;
        }
        Profile::BoundMxf(_) => return Err("unexpected split MXF template profile".into()),
    }
    Ok((id, item))
}

fn standalone_audio(
    item: &mut String,
    media: &Media,
    project_rate: Rational,
    samples: i64,
    bits: i64,
    channels: i64,
    atom: Option<(f64, &[u8])>,
) -> Result<()> {
    let start = atom.map_or(0.0, |(start, _)| start);
    let old = block(item, "BtAudioInfo")?;
    let mut audio = old.clone();
    let audio_id = dbid(&audio)?.to_owned();
    let depth = if bits == 16 { 1 } else { 2 };
    set(
        &mut audio,
        "Clip",
        &clip_cache(media, b"Linear PCM", false)?,
    )?;
    let mut track = vec![
        ("UniqueId", Text(&audio_id)),
        ("StartTime", Double(start)),
        ("SampleRate", Uint(48000)),
        ("NumChannels", Int(channels as i32)),
    ];
    if let Some((_, identity)) = atom {
        track.push(("MXFMetadata", Bytes(identity)));
    }
    track.extend([
        ("IdxTrack", Int(0)),
        ("Duration", Long(samples)),
        ("DbType", Text("BtAudioTrack")),
        ("CodecName", Text("Linear PCM")),
    ]);
    if depth == 2 && atom.is_none() {
        track.push(("ChannelLayout", Int(channels as i32)));
    }
    track.push(("BitDepth", Uint(depth)));
    set(
        &mut audio,
        "TracksBA",
        &hex::encode(qt(&[("0", Bytes(&qt(&track)))])),
    )?;
    replace_once(item, &old, &audio)?;
    let left = qt(&[("MediaRef", Text(&audio_id))]);
    let right = qt(&[("MediaRef", Text(&audio_id)), ("ChannelIdx", Int(1))]);
    let channels_map = if channels == 2 {
        qt(&[("1", Bytes(&right)), ("0", Bytes(&left))])
    } else {
        qt(&[("0", Bytes(&left))])
    };
    let bounds = extent(start, samples as f64 / 48000.0);
    let mut route = Vec::new();
    if channels == 2 {
        route.push(("Type", Uint(0)));
    }
    route.extend([
        ("MediaExtents", Bytes(&bounds)),
        ("CodecName", Text("Linear PCM")),
        ("ChannelVecBA", Bytes(&channels_map)),
        ("BitDepth", Uint(depth)),
    ]);
    let routes = qt(&[("0", Bytes(&qt(&route)))]);
    replace_fields(item, |data| {
        let with_routes =
            replace_bytes(&data, 1, |outer| replace_bytes(outer, 5, |_| Ok(routes)))?;
        replace_bytes(&with_routes, 2, |_| {
            let mut clock = vec![9];
            clock.extend_from_slice(&fps(project_rate).to_le_bytes());
            Ok(clock)
        })
    })
}

fn sequence_fields(sequence: &str) -> Result<String> {
    let end = sequence
        .find("<UniqueSequenceId>")
        .ok_or("missing sequence identity")?;
    tag(&sequence[..end], "FieldsBlob")
}

struct TimelineDonors<'a> {
    pool: &'a str,
    container: &'a str,
    fairlight: Option<&'a Mixer>,
}

fn write_timeline<'a>(
    timeline: &'a Timeline,
    rate: Rational,
    bin: &str,
    media: &[Media],
    media_ids: &[String],
    donors: &TimelineDonors<'_>,
    edit_metadata: &mut Vec<markers::EditMetadata<'a>>,
) -> Result<(String, String, String, String, String)> {
    let mut ids = new_ids(donors.pool)?;
    for (old, new) in new_ids(donors.container)? {
        if ids.insert(old, new).is_some() {
            return Err("colliding timeline template identities".into());
        }
    }
    let mut pool = remap(donors.pool.to_owned(), &ids);
    let mut container = remap(donors.container.to_owned(), &ids);
    set(&mut pool, "MpFolder", bin)?;
    prefix_name(&mut pool, &timeline.name)?;
    if let Some(marks) = timeline.marks {
        for (tag, frame) in marks.fields() {
            set(&mut pool, tag, &frame.to_string())?;
        }
    }
    replace_fields(&mut pool, |data| Ok(remap_blob(data, &ids)))?;
    let (old, mut handle, handle_id) = if timeline.compound {
        (String::new(), String::new(), String::new())
    } else {
        set(&mut pool, "CurPlayheadPosition", "0")?;
        let old = block(&pool, "Sm2Timeline")?;
        let mut handle = old.clone();
        let head_end = handle.find("<Sequence>").ok_or("missing timeline sequence wrapper")?;
        let mut head = handle[..head_end].to_owned();
        set(&mut head, "Name", &xml(&timeline.name))?;
        handle.replace_range(..head_end, &head);
        let id = dbid(&handle)?.to_owned();
        (old, handle, id)
    };
    let old_sequence = block(if timeline.compound { &pool } else { &handle }, "Sm2Sequence")?;
    let mut sequence = old_sequence.clone();
    let sequence_id = dbid(&sequence)?.to_owned();
    let rate_hex = hex::encode(rate_bytes(rate));
    let mut timeline_clock = rate_bytes(rate);
    timeline_clock[15] = u8::from(is_2997(rate));
    set(&mut sequence, "FrameRate", &hex::encode(timeline_clock))?;
    set(
        &mut sequence,
        "MediaExtents",
        &hex::encode(extent(
            timeline.origin as f64 / fps(rate),
            timeline.length as f64 / fps(rate),
        )),
    )?;
    let fields_end = sequence
        .find("<UniqueSequenceId>")
        .ok_or("missing sequence identity")?;
    let mut fields_head = sequence[..fields_end].to_owned();
    let fields = sequence_fields(&sequence)?;
    let fields = if let Some(mixer) = timeline.mixer {
        Mixer::decode(&fields)?
            .ok_or("missing qualified Fairlight mixer in timeline template")?
            .write_direct(donors.fairlight, mixer)?
    } else if timeline.faders != Default::default() {
        Mixer::decode(&fields)?
            .ok_or("missing static Fairlight fader template")?
            .write_linked_faders(timeline.faders)?
    } else {
        fields
    };
    let fields = if let Some(tracks) = timeline.track_state.as_ref().filter(|s| {
        s.0[1..].iter().any(|track| !track.name.is_empty() || track.locked)
    }) {
        Mixer::decode(&fields)?.ok_or("missing Fairlight track-state template")?
            .write_track_state(&tracks.0[1..])?
    } else {
        fields
    };
    set(
        &mut fields_head,
        "FieldsBlob",
        &hex::encode(remap_blob(hex::decode(fields)?, &ids)),
    )?;
    sequence.replace_range(..fields_end, &fields_head);
    if timeline.compound {
        replace_once(&mut pool, &old_sequence, &sequence)?;
        let old = tag(&pool, "VideoMetadata")?;
        let new = remap_blob(hex::decode(old)?, &ids);
        set(&mut pool, "VideoMetadata", &hex::encode(new))?;
    } else {
        replace_once(&mut handle, &old_sequence, &sequence)?;
        replace_once(&mut pool, &old, &handle)?;
    }
    let second_picture = !timeline.lanes[3].is_empty();
    let third_mono = !timeline.lanes[4].is_empty();
    if second_picture && !third_mono {
        let video = block(&container, "VideoTrackVec")?;
        let track = block(&video, "Sm2TiTrack")?;
        let mut extra = track.clone();
        replace_once(&mut extra, dbid(&track)?, &uid())?;
        let mut expanded = video.clone();
        replace_once(&mut expanded, "</VideoTrackVec>", &format!("<Element>{extra}</Element></VideoTrackVec>"))?;
        replace_once(&mut container, &video, &expanded)?;
    }
    let item_lists = Regex::new(r"(?s)<Items(?:/>|>.*?</Items>)")?;
    let spans: Vec<_> = item_lists
        .find_iter(&container)
        .map(|m| m.range())
        .collect();
    let lane_order: &[usize] = if timeline.compound || timeline.stereo && timeline.lanes[2].is_empty() {
        &[0, 1]
    } else if third_mono {
        &[0, 3, 1, 2, 4]
    } else if second_picture {
        &[0, 3, 1, 2]
    } else {
        &[0, 1, 2]
    };
    if spans.len() != lane_order.len() {
        return Err("expected V1 and two native mono track templates".into());
    }
    for (&lane, span) in lane_order.iter().zip(spans).rev() {
        let base = if timeline.lanes[lane].is_empty() {
            ""
        } else {
            blocks(if matches!(lane, 0 | 3) { "Sm2TiVideoClip" } else { "Sm2TiAudioClip" })?
                .find(&container[span.clone()])
                .ok_or("missing native edit template")?.as_str()
        };
        let mut list = String::from("<Items>");
        for edit in &timeline.lanes[lane] {
            if let Some(dissolve) = timeline
                .dissolves
                .iter()
                .find(|dissolve| lane == usize::from(dissolve.audio()) && edit.start == dissolve.cut)
            {
                write_dissolve(&mut list, timeline.origin, dissolve)?;
            }
            let source = &media[edit.media];
            let mut clip = base.to_owned();
            let old_id = dbid(&clip)?.to_owned();
            let clip_id = uid();
            replace_once(&mut clip, &old_id, &clip_id)?;
            if edit.label.is_some() || !edit.markers.is_empty() {
                edit_metadata.push(markers::EditMetadata {
                    owner: clip_id,
                    label: edit.label,
                    markers: &edit.markers,
                });
            }
            set(&mut clip, "Name", &xml(&source.name))?;
            set(&mut clip, "MediaRef", &media_ids[edit.media])?;
            set(&mut clip, "MediaFilePath", &xml(utf8_path(&source.path)?))?;
            if let Timing::MixedRate(cadence) = &edit.timing {
                set(&mut clip, "MediaFrameRate", &hex::encode(rate_bytes(cadence.source_rate())))?;
            } else {
                set(&mut clip, "MediaFrameRate", &rate_hex)?;
            }
            set(
                &mut clip,
                "Start",
                &(timeline.origin + edit.start).to_string(),
            )?;
            set(&mut clip, "Duration", &edit.length.to_string())?;
            if let Some(video) = &edit.video {
                set(&mut clip, "EffectFiltersBA", &video.encoded)?;
            }
            if edit.input_sizing == Some(crate::input_scaling::Sizing::CenterCrop) {
                replace_fields(&mut clip, |fields| crate::input_scaling::crop_fields(&fields))?;
                set(&mut clip, "EffectFiltersBA", crate::input_scaling::CROP_FILTERS)?;
            }
            if let Some(envelope) = edit.envelope {
                set(&mut clip, "EffectFiltersBA", &envelope.encode()?)?;
            }
            let atom = match &source.profile {
                Profile::BoundMxf(binding) => Some(binding),
                _ => None,
            };
            let end = match &source.profile {
                Profile::Mov { frames, .. } => {
                    if let Some(start) = source.profile.movie_timecode() {
                        set(&mut clip, "MediaStartTime", &(start as f64 / fps(rate)).to_string())?;
                    }
                    (frames - 1) as f64 / fps(rate)
                }
                Profile::Wav { samples, .. } => {
                    (samples - samples_per_frame(rate)?) as f64 / 48000.0
                }
                Profile::MonoAtom { frames, start, .. } => {
                    set(&mut clip, "MediaStartTime", &seconds(*start).to_string())?;
                    seconds(frames - 1)
                }
                Profile::BoundMxf(binding) => {
                    set(
                        &mut clip,
                        "MediaStartTime",
                        &seconds(binding.timecode_frames).to_string(),
                    )?;
                    seconds(binding.frames - 1)
                }
            };
            let (saved_in, map) = match (&edit.timing, &source.profile) {
                (Timing::Linear(linear), Profile::Mov { .. }) => {
                    set(&mut clip, "EffectFiltersBA", crate::retime::NEAREST)?;
                    (linear.record_in.to_string(), linear.encode(&uid()))
                }
                (Timing::Linear(_), _) => return Err("Resolve Linear retime requires a plain linked MOV edit".into()),
                (Timing::Constant(speed), Profile::Mov { frames, .. }) => {
                    let saved_in = speed.record_source_in(edit.source_in, edit.length, *frames)
                        .ok_or("invalid Resolve retime source trim")?;
                    set(&mut clip, "EffectFiltersBA", crate::retime::NEAREST)?;
                    (saved_in, speed.encode(*frames, &uid())?)
                }
                (Timing::Constant(_), _) => return Err("Resolve picture speed requires linked MOV media".into()),
                (Timing::Hold, Profile::Mov { frames, .. }) => {
                    set(&mut clip, "EffectFiltersBA", crate::retime::NEAREST)?;
                    (crate::retime::HOLD_IN.to_owned(), crate::retime::encode_hold(edit.source_in, *frames, &uid())?)
                }
                (Timing::MixedRate(cadence), Profile::Mov { frames, .. }) => {
                    let saved_in = cadence.encode_in(edit.source_in)
                        .ok_or("mixed-rate picture cadence is not qualified for Resolve writing")?;
                    (saved_in, cadence.time_map(*frames))
                }
                (Timing::MixedRate(_), _) => return Err("Resolve mixed-rate picture requires linked MOV media".into()),
                (Timing::Straight | Timing::Hold, _) => {
                    let mut map = vec![2];
                    map.extend_from_slice(&end.to_be_bytes());
                    (edit.source_in.to_string(), map)
                }
            };
            set(&mut clip, "In", &saved_in)?;
            set(&mut clip, "MediaTimemapBA", &hex::encode(map))?;
            if let Some((_, body)) = &edit.grade {
                write_grade(&mut clip, body)?;
            }
            if let Some(channel) = edit.channel {
                let (selector, payload) = if let Some(native) = &source.native {
                    let saved = native.selectors.as_ref().and_then(|selectors| selectors.get(channel))
                        .ok_or("missing verified native audio selector")?;
                    (channel, saved.clone())
                } else {
                    (if atom.is_some() { channel } else { 0 }, hex::encode(mono_selector(channel, atom.is_some())?))
                };
                set(&mut clip, "VirtualAudioTrackBA", &payload)?;
                set(&mut clip, "MediaTrackIdx", &selector.to_string())?;
            } else if timeline.stereo && lane == 1 {
                set(&mut clip, "VirtualAudioTrackBA", crate::stereo::SELECTOR)?;
                set(&mut clip, "MediaTrackIdx", "0")?;
            }
            if list.len().saturating_add(clip.len()).saturating_add(32) as u64 > MAX_XML {
                return Err("timeline XML exceeds its construction budget".into());
            }
            list.push_str("<Element>");
            list.push_str(&clip);
            list.push_str("</Element>");
        }
        if lane == 0 {
            if let Some(solid) = &timeline.solid {
                use std::fmt::Write as _;
                write!(
                    list,
                    concat!(
                        "<Element><Sm2TiGenerator DbId=\"{}\"><FieldsBlob/><PrettyType>Solid Color</PrettyType>",
                        "<Name>Solid Color</Name><Start>{}</Start><Duration>{}</Duration><LinkedItemSync/>",
                        "<WasDisbanded>false</WasDisbanded><MarkersBA/><UiMemento>0</UiMemento><Flags>0</Flags>",
                        "<PriorityIndex>0</PriorityIndex><EffectFiltersBA>{}</EffectFiltersBA>",
                        "<ImportExportMetadataBA/><RenderTextEnabled>true</RenderTextEnabled>",
                        "<RenderTextGanged>true</RenderTextGanged><RenderTextPrefixed>true</RenderTextPrefixed>",
                        "<In/></Sm2TiGenerator></Element>",
                    ),
                    uid(), timeline.origin + solid.start, solid.duration, solid.effect(),
                )?;
            }
        }
        list.push_str("</Items>");
        container.replace_range(span, &list);
    }
    if timeline.track_state.is_some() || timeline.faders.muted != [false; 2] {
        let neutral = crate::track_state::TrackState(Default::default());
        let state = timeline.track_state.as_ref().unwrap_or(&neutral);
        let pattern = blocks("Sm2TiTrack")?;
        let spans: Vec<_> = pattern.find_iter(&container).map(|row| row.range()).collect();
        if spans.len() != state.0.len() {
            return Err("unexpected Resolve track-state template layout".into());
        }
        for ((span, track), muted) in spans.into_iter().zip(&state.0)
            .zip([false, timeline.faders.muted[0], timeline.faders.muted[1]]).rev()
        {
            let mut xml_track = container[span.clone()].to_owned();
            let at = xml_track.find("<Items").ok_or("missing Resolve track items")?;
            if track.locked || track.disabled || muted {
                let mut head = xml_track[..at].to_owned();
                let flags: u32 = tag(&head, "Flags")?.parse()?;
                let flags = flags | u32::from(track.locked) | (u32::from(track.disabled || muted) << 1);
                set(&mut head, "Flags", &flags.to_string())?;
                xml_track.replace_range(..at, &head);
            }
            if !track.name.is_empty() {
                set(&mut xml_track, "UserDefinedName", &xml(&track.name))?;
            }
            container.replace_range(span, &xml_track);
        }
    }
    if !timeline.captions.is_empty() {
        write_captions(&mut container, timeline, &sequence_id)?;
    }
    Ok((
        handle_id,
        sequence_id,
        dbid(&container)?.to_owned(),
        pool,
        container,
    ))
}

fn write_nested_uses(
    parent: &Timeline,
    project: &PreparedProject,
    pools: &[Option<String>],
    template: &str,
    audio_template: Option<&str>,
    container: &mut String,
    remaining_xml: &mut usize,
) -> Result<()> {
    let old_video = block(container, "VideoTrackVec")?;
    let old_track = block(&old_video, "Sm2TiTrack")?;
    let clip_pattern = blocks("Sm2TiVideoClip")?;
    let clips = clip_pattern.find_iter(&old_track).map(|m| m.as_str());
    let mut edits: Vec<_> = parent.lanes[0].iter().map(|edit| edit.start).zip(clips)
        .map(|(start, clip)| (start, clip.to_owned())).collect();
    if edits.len() != parent.lanes[0].len() {
        return Err("generated Resolve parent V1 layout differs from preparation".into());
    }
    let mut nested_audio = String::new();
    for usage in &parent.nests {
        let child = &project.timelines[usage.child];
        let pool = pools[usage.child].as_ref().ok_or("missing generated child pool identity")?;
        let mut clip = write_nest_clip(template, parent, child, usage, pool)?;
        if usage.stereo_audio.is_some() {
            let audio_template = audio_template.ok_or("missing stereo nested audio template")?;
            let mut audio = write_nest_clip(audio_template, parent, child, usage, pool)?;
            let video_id = dbid(&clip)?.to_owned();
            let audio_id = dbid(&audio)?.to_owned();
            replace_fields(&mut clip, |_| Ok(crate::nests::pair_fields(&audio_id, false)))?;
            replace_fields(&mut audio, |_| Ok(crate::nests::pair_fields(&video_id, true)))?;
            *remaining_xml = remaining_xml.checked_sub(audio.len() + 19)
                .ok_or("nested edit XML exceeds its construction budget")?;
            nested_audio.push_str("<Element>");
            nested_audio.push_str(&audio);
            nested_audio.push_str("</Element>");
        }
        *remaining_xml = remaining_xml.checked_sub(clip.len() + 19)
            .ok_or("nested edit XML exceeds its construction budget")?;
        edits.push((usage.start, clip));
    }
    edits.sort_unstable_by_key(|(start, _)| *start);
    let list = edits.into_iter().map(|(_, xml)| format!("<Element>{xml}</Element>")).collect::<String>();
    let mut track = old_track.clone();
    container_field(&mut track, "Items", &list)?;
    let mut video = old_video.clone();
    replace_once(&mut video, &old_track, &track)?;
    replace_once(container, &old_video, &video)?;
    if !nested_audio.is_empty() {
        let original = block(container, "AudioTrackVec")?;
        let mut audio = original.clone();
        container_field(&mut audio, "Items", &nested_audio)?;
        replace_once(container, &original, &audio)?;
    }
    Ok(())
}

fn write_nest_clip(
    template: &str,
    parent: &Timeline,
    child: &Timeline,
    usage: &crate::nests::Use,
    pool: &str,
) -> Result<String> {
    let mut clip = remap(template.to_owned(), &new_ids(template)?);
    for (key, value) in [
        ("Name", xml(&child.name)),
        ("Start", (parent.origin + usage.start).to_string()),
        ("Duration", usage.length.to_string()),
        ("In", usage.source_in.to_string()),
        ("MediaRef", pool.to_owned()),
        ("MediaStartTime", (usage.origin as f64 / 24.0).to_string()),
        ("MediaTimemapBA", hex::encode(crate::nests::compact_map(usage.child_length))),
    ] {
        set(&mut clip, key, &value)?;
    }
    Ok(clip)
}

fn write_grade(clip: &mut String, body: &str) -> Result<()> {
    use crate::binary::Qt;
    let active = uid();
    let table = uid();
    let fields = hex::encode(qt(&[("LastChangedTime", Qt::Long(0)), ("ActiveVersionType", Qt::Int(0))]));
    let grade = format!(
        concat!(
            "<pLmVerTable><ListMgt::LmVersionTable DbId=\"{table}\"><FieldsBlob>{fields}</FieldsBlob>",
            "<VerType>0</VerType><pActive>{active}</pActive><Locals><Element>",
            "<ListMgt::LmVersion DbId=\"{active}\"><FieldsBlob/><Name>Version 1</Name>",
            "<HasCorrection>true</HasCorrection><VerType>0</VerType><ImplVersion>1</ImplVersion>",
            "<IncludedInRecording>true</IncludedInRecording><FlatPassEnabled>false</FlatPassEnabled>",
            "<RGBAOutputEnabled>false</RGBAOutputEnabled><Body>{body}</Body>",
            "<UseVersionClipProcParams>true</UseVersionClipProcParams></ListMgt::LmVersion>",
            "</Element></Locals><DbSavedTime>0</DbSavedTime></ListMgt::LmVersionTable></pLmVerTable>"
        ),
        table=table, fields=fields, active=active, body=body
    );
    if clip.contains("<pLmVerTable>") {
        return Err("Resolve skeleton already contains a clip grade".into());
    }
    let anchor = clip.find("<LastChangedTime>").ok_or("missing Resolve grade insertion anchor")?;
    clip.insert_str(anchor, &grade);
    Ok(())
}

fn write_captions(container: &mut String, timeline: &Timeline, sequence_id: &str) -> Result<()> {
    use std::fmt::Write as _;
    let mut track = format!(
        "<SubtitleTrackVec><Element><Sm2TiTrack DbId=\"{}\"><FieldsBlob>{}</FieldsBlob>\
         <Type>2</Type><SubType>0</SubType><Flags>0</Flags><Sequence>{sequence_id}</Sequence><Items>",
        uid(), crate::captions::TRACK_FIELDS,
    );
    for caption in &timeline.captions {
        if track.len().saturating_add(caption.text.len().saturating_mul(6) + 640) as u64 > MAX_XML {
            return Err("Resolve subtitle track exceeds its construction budget".into());
        }
        write!(
            track,
            concat!(
                "<Element><Sm2TiGenerator DbId=\"{}\"><FieldsBlob/><PrettyType>Subtitle</PrettyType>",
                "<Name>{}</Name><Start>{}</Start><Duration>{}</Duration><LinkedItemSync/>",
                "<WasDisbanded>false</WasDisbanded><MarkersBA/><UiMemento>0</UiMemento><Flags>0</Flags>",
                "<PriorityIndex>0</PriorityIndex><EffectFiltersBA/><ImportExportMetadataBA/>",
                "<RenderTextEnabled>true</RenderTextEnabled><RenderTextGanged>true</RenderTextGanged>",
                "<RenderTextPrefixed>true</RenderTextPrefixed><In/></Sm2TiGenerator></Element>",
            ),
            uid(), xml(&caption.text), timeline.origin + caption.start, caption.length,
        )?;
    }
    track.push_str("</Items><UserDefinedName/><LayersVec/></Sm2TiTrack></Element></SubtitleTrackVec>");
    let empty = one(&Regex::new(r"<SubtitleTrackVec(?:\s*/>|>\s*</SubtitleTrackVec>)")?, container)?;
    container.replace_range(empty.range(), &track);
    Ok(())
}

fn write_dissolve(list: &mut String, origin: i64, transition: &Dissolve) -> Result<()> {
    use std::fmt::Write as _;
    // Resolve keeps the visible edit ranges and stores overlap on the transition.
    let start = origin + transition.window().start;
    write!(
        list,
        concat!(
            "<Element><Sm2TiTransition DbId=\"{id}\">",
            "<FieldsBlob>{fields}</FieldsBlob><PrettyType>{kind}</PrettyType>",
            "<Name>Transition</Name><Start>{start}</Start><Duration>{duration}</Duration>",
            "<LinkedItemSync/><WasDisbanded>false</WasDisbanded><MarkersBA/>",
            "<UiMemento>0</UiMemento><Flags>0</Flags><PriorityIndex>0</PriorityIndex>",
            "<EffectFiltersBA>{effect}</EffectFiltersBA><ImportExportMetadataBA/>",
            "<RenderTextEnabled>true</RenderTextEnabled><RenderTextGanged>true</RenderTextGanged>",
            "<RenderTextPrefixed>true</RenderTextPrefixed>",
            "<AlignmentType>{alignment}</AlignmentType><Position>{position}</Position>",
            "</Sm2TiTransition></Element>"
        ),
        id = uid(),
        fields = if transition.audio() { dissolve::AUDIO_FIELDS } else { dissolve::FIELDS },
        kind = transition.style.native_label(),
        start = start,
        duration = transition.duration,
        effect = if transition.audio() { String::new() } else { hex::encode(dissolve::video_effect(transition.duration)?) },
        alignment = transition.alignment(),
        position = dissolve::POSITION,
    )
    .expect("writing to a String cannot fail");
    Ok(())
}

fn folder_path(index: usize, project: &PreparedProject) -> Result<String> {
    let mut path = Vec::new();
    let mut current = index;
    while let Some(parent) = project.bins[current].parent {
        if path.len() == 16 {
            return Err("Resolve bin nesting exceeds project profile".into());
        }
        let sibling = project.bins[..current]
            .iter()
            .filter(|bin| bin.parent == Some(parent))
            .count();
        let name: String = project.bins[current]
            .name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '_' | '-') {
                    ch
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        path.push(format!("{sibling:03}_{name}"));
        current = parent;
    }
    let mut result = String::from("MediaPool/Master");
    for part in path.iter().rev() {
        result.push('/');
        result.push_str(part);
    }
    Ok(result + "/MpFolder.xml")
}

struct Identities {
    bins: Vec<String>,
    media: Vec<String>,
    sequences: Vec<String>,
    timeline_pools: Vec<Option<String>>,
    group: Option<String>,
}

fn build(
    p: &PreparedProject,
    layout: &Members,
    profile: &Members,
    mxf_donors: [Option<&str>; 2],
    group_template: Option<&Path>,
    donors: &TimelineDonors<'_>,
    wider_layout: Option<&Members>,
    nest_template: Option<&Members>,
) -> Result<(Members, Identities)> {
    let root = member(profile, POOL)?;
    let root_id = dbid(root)?.to_owned();
    let media_pool_id = tag(root, "MediaPool")?;
    let mut bin_ids = vec![root_id];
    bin_ids.extend((1..p.bins.len()).map(|_| uid()));
    let mut children = vec![String::new(); p.bins.len()];
    let mut media_ids = Vec::new();
    let mut remaining_xml = MAX_XML as usize;
    let donor = member(layout, MEDIA_DONOR)?;
    let wide_pool = wider_layout.map(|members|
        named(member(members, POOL)?, "Sm2MpTimelineClip", "Native lanes")).transpose()?;
    let wider_donors = match (wider_layout, wide_pool.as_deref()) {
        (Some(members), Some(pool)) => Some(TimelineDonors {
            pool, container: member(members, THREE_MONO_SEQUENCE)?, fairlight: None,
        }),
        _ => None,
    };
    let stereo_nest = p.timelines.iter().flat_map(|row| &row.nests).any(|usage| usage.stereo_audio.is_some());
    let compound_template = nest_template.filter(|_| !stereo_nest);
    let compound_pool = compound_template.map(|members| block(member(members, POOL)?, "Sm2MpCompoundClip")).transpose()?;
    let compound_donors = match (compound_template, compound_pool.as_deref()) {
        (Some(members), Some(pool)) => Some(TimelineDonors {
            pool, container: member(members, NEST_COMPOUND)?, fairlight: None,
        }),
        _ => None,
    };
    let (nest_edit, nest_audio) = if let Some(members) = nest_template {
        if stereo_nest {
            let parent = member(members, STEREO_NEST_PARENT)?;
            (
                Some(named(parent, "Sm2TiVideoClip", "Child A")?),
                Some(named(parent, "Sm2TiAudioClip", "Child A")?),
            )
        } else {
            (Some(named(member(members, NEST_PARENT)?, "Sm2TiVideoClip", "Witness A")?), None)
        }
    } else {
        (None, None)
    };
    let stereo_sources: HashSet<_> = p.timelines.iter().filter(|row| row.stereo)
        .flat_map(|row| row.lanes[1].iter().map(|edit| edit.media))
        .collect();
    for (index, media) in p.media.iter().enumerate() {
        let (id, item) = pool_item(
            donor, media, &bin_ids[media.bin], p.rate, mxf_donors, stereo_sources.contains(&index),
        )?;
        remaining_xml = remaining_xml
            .checked_sub(item.len() + 19)
            .ok_or("media XML exceeds its construction budget")?;
        media_ids.push(id);
        children[media.bin].push_str(&format!("<Element>{item}</Element>"));
    }
    let mut members = Members::new();
    let mut handles = String::new();
    let mut sequence_ids = Vec::new();
    let mut timeline_pools = Vec::new();
    let mut timeline_containers = Vec::new();
    let mut group_id = None;
    let mut edit_metadata = Vec::new();
    let mut project = member(profile, PROJECT)?.to_owned();
    set(&mut project, "ProjectName", &xml(&p.name))?;
    set(
        &mut project,
        "CurrentTimelineIndex",
        &p.active_timeline_index.to_string(),
    )?;
    container_field(&mut project, "LocableBlobSet", "")?;
    for timeline in &p.timelines {
        if let Some(group) = &timeline.group {
            let built = super::group::build_turnover_group(
                &group.prepared,
                group_template.ok_or("Group project requires its pinned Group template")?,
                &bin_ids[group.bin],
                &bin_ids[timeline.bin],
                group.cameras.map(|index| media_ids[index].as_str()),
            )?;
            let added = built.pool.len()
                + built.timeline.len()
                + 38
                + built.containers.values().map(String::len).sum::<usize>();
            remaining_xml = remaining_xml
                .checked_sub(added)
                .ok_or("Group XML exceeds its construction budget")?;
            children[group.bin].push_str(&format!("<Element>{}</Element>", built.pool));
            children[timeline.bin].push_str(&format!("<Element>{}</Element>", built.timeline));
            handles.push_str(&format!("<Element>{}</Element>", built.handle));
            sequence_ids.push(built.sequence);
            timeline_pools.push(None);
            timeline_containers.push(None);
            group_id = Some(built.id);
            for (name, contents) in built.containers {
                if members.insert(name, contents).is_some() {
                    return Err("Group sequence container identity collision".into());
                }
            }
            continue;
        }
        let donors = if timeline.compound {
            compound_donors.as_ref().ok_or("missing native compound template")?
        } else if timeline.lanes[4].is_empty() {
            donors
        } else {
            wider_donors.as_ref().ok_or("missing three-mono timeline layout")?
        };
        let (handle, sequence, container_id, pool, container) = write_timeline(
            timeline,
            p.rate,
            &bin_ids[timeline.bin],
            &p.media,
            &media_ids,
            donors,
            &mut edit_metadata,
        )?;
        remaining_xml = remaining_xml
            .checked_sub(pool.len() + container.len() + 19)
            .ok_or("project XML exceeds its construction budget")?;
        children[timeline.bin].push_str(&format!("<Element>{pool}</Element>"));
        if !timeline.compound {
            handles.push_str(&format!("<Element>{handle}</Element>"));
            markers::write(&mut project, &sequence, &timeline.markers)?;
        }
        timeline_pools.push(Some(dbid(&pool)?.to_owned()));
        let path = format!("SeqContainer/{container_id}.xml");
        timeline_containers.push(Some(path.clone()));
        sequence_ids.push(sequence);
        if members
            .insert(path, container)
            .is_some()
        {
            return Err("sequence container identity collision".into());
        }
    }
    if let Some(template) = nest_edit.as_deref() {
        for (timeline, path) in p.timelines.iter().zip(timeline_containers) {
            if timeline.nests.is_empty() { continue; }
            let target = path.and_then(|path| members.get_mut(&path)).ok_or("missing parent container")?;
            write_nested_uses(timeline, p, &timeline_pools, template, nest_audio.as_deref(), target, &mut remaining_xml)?;
        }
    }
    markers::write_edits(&mut project, &edit_metadata)?;
    if !p.pool_markers.is_empty() {
        let rows = media_ids.iter().enumerate().filter_map(|(index, id)|
            p.pool_markers.get(&index).map(|marks| (id.as_str(), marks.as_slice())));
        markers::write_pool(&mut project, rows)?;
    }
    container_field(&mut project, "TimelineHandleVec", &handles)?;
    members.insert(PROJECT.into(), project);
    members.insert("Gallery.xml".into(), member(profile, "Gallery.xml")?.into());
    for (index, bin) in p.bins.iter().enumerate() {
        let mut folder = if let Some(parent) = bin.parent {
            let mut folder = donor.to_owned();
            let old_id = dbid(&folder)?.to_owned();
            container_field(&mut folder, "MediaVec", "")?;
            replace_once(&mut folder, &old_id, &bin_ids[index])?;
            set(&mut folder, "MpFolder", &bin_ids[parent])?;
            set(&mut folder, "MediaPool", &media_pool_id)?;
            set(&mut folder, "UniqueMediaPoolItemId", &uid())?;
            folder
        } else {
            root.to_owned()
        };
        container_field(&mut folder, "MediaVec", &children[index])?;
        let end = folder
            .find("<MediaVec>")
            .ok_or("missing folder member list")?;
        let mut head = folder[..end].to_owned();
        set(&mut head, "Name", &xml(&bin.name))?;
        folder.replace_range(..end, &head);
        let children: Vec<_> = p
            .bins
            .iter()
            .enumerate()
            .filter(|(_, row)| row.parent == Some(index))
            .map(|(child, _)| bin_ids[child].as_str())
            .enumerate()
            .map(|(ordinal, id)| (ordinal.to_string(), id))
            .collect();
        let route: Vec<_> = children
            .iter()
            .rev()
            .map(|(key, id)| (key.as_str(), Text(id)))
            .collect();
        replace_fields(&mut folder, |data| {
            crate::binary::rewrite_bytes(&data, 2, |_| Ok((!route.is_empty()).then(|| qt(&route))))
        })?;
        members.insert(folder_path(index, p)?, folder);
    }
    if members.values().map(|s| s.len() as u64).sum::<u64>() > MAX_XML {
        return Err("generated project XML exceeds its budget".into());
    }
    Ok((
        members,
        Identities {
            bins: bin_ids,
            media: media_ids,
            sequences: sequence_ids,
            timeline_pools,
            group: group_id,
        },
    ))
}

fn verify_stage(p: &PreparedProject, ids: &Identities, path: &Path) -> Result<()> {
    let graph = crate::source::read(path)?;
    if graph.sequences.len() != p.timelines.len()
        || graph.items.len() != p.media.len()
        || graph.bins.len() != p.bins.len()
    {
        return Err("generated Resolve reference counts differ from preparation".into());
    }
    let active = graph
        .sidecar(crate::source::ACTIVE_TIMELINE_ADAPTER, &ids.bins[0])
        .ok_or("generated Resolve active timeline receipt missing")?;
    let expected_index = Value::Int(i64::try_from(p.active_timeline_index)?);
    let registered = p.timelines.iter().zip(&ids.sequences)
        .filter(|(timeline, _)| !timeline.compound).map(|(_, id)| id);
    if !graph.sequences.iter().filter(|s| !crate::nests::compound(&graph,&s.id))
        .map(|s| &s.id).eq(registered.clone())
    {
        return Err("generated Resolve registered timeline order differs from preparation".into());
    }
    let expected_id = registered.clone().nth(p.active_timeline_index)
        .ok_or("generated Resolve active timeline index is absent")?;
    if active.get("saved_index") != Some(&expected_index)
        || !matches!(active.get("sequence_id"), Some(Value::Str(id)) if id == expected_id)
        || active.get("unavailable_reason") != Some(&Value::Null)
    {
        return Err("generated Resolve active timeline differs from preparation".into());
    }
    let readbacks: Vec<_> = ids.sequences.iter().map(|id|
        graph.sequences.iter().find(|sequence| sequence.id == *id)
            .ok_or("generated Resolve sequence ID is absent")).collect::<std::result::Result<_, _>>()?;
    let mut dissolves = Vec::with_capacity(p.timelines.len());
    let mut nested_ids = std::collections::HashSet::new();
    for (timeline, &read) in p.timelines.iter().zip(&readbacks) {
        use crate::nests::{int, stereo_pairs, string, ADAPTER, PROFILE, STEREO_PROFILE};
        let audio_uses = timeline.nests.iter().filter(|usage| usage.stereo_audio.is_some());
        let count = timeline.nests.len() + audio_uses.clone().count();
        match graph.sidecar(ADAPTER, &read.id) {
            None if timeline.nests.is_empty() => {}
            Some(Value::Arr(rows)) if rows.len() == count => {
                stereo_pairs(rows)?;
                let expected_uses = timeline.nests.iter().map(|usage| (usage, false))
                    .chain(audio_uses.map(|usage| (usage, true)));
                for (row, (expected, is_audio)) in rows.iter().zip(expected_uses) {
                    let child = &ids.sequences[expected.child];
                    let pool = ids.timeline_pools[expected.child].as_deref().ok_or("missing staged child pool")?;
                    let id = string(row,"id").ok_or("missing staged nested occurrence ID")?;
                    let profile = if expected.stereo_audio.is_some() { STEREO_PROFILE } else { PROFILE };
                    if !nested_ids.insert((read.id.as_str(), id))
                        || string(row,"profile") != Some(profile)
                        || string(row,"child_sequence") != Some(child.as_str())
                        || string(row,"pool_id") != Some(pool)
                        || string(row,"kind") != Some(expected.kind.saved())
                        || string(row,"track_kind") != Some(if is_audio { "audio" } else { "video" })
                        || int(row,"track") != Some(1)
                        || string(row,"record_rate") != Some("24") || string(row,"child_rate") != Some("24")
                        || int(row,"child_origin_frames") != Some(expected.origin)
                        || int(row,"record_start_frames") != Some(expected.start)
                        || int(row,"record_duration_frames") != Some(expected.length)
                        || int(row,"saved_source_in_frames") != Some(expected.source_in)
                    {
                        return Err("generated Resolve nested binding or trim differs from preparation".into());
                    }
                }
            }
            _ => return Err("generated Resolve nested use count differs from preparation".into()),
        }
        let saved = dissolve::prepare_all_native(&graph, read)?;
        if saved.len() != timeline.dissolves.len() || !timeline.dissolves.iter().all(|expected| {
            saved.iter().any(|actual| actual.prepared.style == expected.style
                && actual.prepared.duration == expected.duration
                && actual.prepared.cut == expected.cut && actual.prepared.before == expected.before)
        }) {
            return Err("generated Resolve dissolve differs from preparation".into());
        }
        dissolves.push(saved);
    }
    for (index, bin) in p.bins.iter().enumerate() {
        let actual = graph
            .bins
            .iter()
            .find(|row| row.id == ids.bins[index])
            .ok_or("generated bin missing")?;
        let expected_items: Vec<_> = p
            .media
            .iter()
            .zip(&ids.media)
            .filter(|(media, _)| media.bin == index)
            .map(|(_, id)| id)
            .collect();
        if actual.name != bin.name
            || actual.parent.as_ref() != bin.parent.map(|parent| &ids.bins[parent])
            || !actual.items.iter().eq(expected_items)
        {
            return Err("generated Resolve bin ownership differs from preparation".into());
        }
        let expected_children = p
            .bins
            .iter()
            .zip(&ids.bins)
            .filter(|(child, _)| child.parent == Some(index))
            .map(|(_, id)| id);
        let actual_children = graph
            .bins
            .iter()
            .filter(|child| child.parent.as_ref() == Some(&actual.id))
            .map(|child| &child.id);
        let has_children = expected_children.clone().next().is_some();
        let order = graph.sidecar(crate::source::BIN_ORDER_ADAPTER, &actual.id);
        let saved_order = order.is_some_and(|fields| {
            matches!(fields.get("source"), Some(Value::Str(source)) if source == "fields_blob")
                && fields.get("archive_order_matches") == Some(&Value::Bool(true))
        });
        if !actual_children.eq(expected_children) || (has_children && !saved_order) {
            return Err("generated Resolve child bin order differs from preparation".into());
        }
    }
    for (index, (media, id)) in p.media.iter().zip(&ids.media).enumerate() {
        let item = graph.items.iter().find(|item| item.id == *id)
            .ok_or("generated Resolve media item absent")?;
        if let Some(expected) = p.pool_markers.get(&index) {
            let (rate, frames) = media.profile.pool_marker_grid(p.rate)?
                .ok_or("generated Resolve source markers require qualified linked media")?;
            let (actual, omitted) = crate::prepare::prepare_pool_markers(&graph, item, rate, frames)?;
            if !omitted.is_empty() || &actual != expected
                || graph.sidecar(crate::source::POOL_MARKER_ADAPTER, id).is_none()
            {
                return Err("generated Resolve source markers differ from preparation".into());
            }
        } else if !item.markers.is_empty()
            || graph.sidecar(crate::source::POOL_MARKER_ADAPTER, id).is_some()
        {
            return Err("generated Resolve media has unexpected source markers".into());
        }
        if let Some(native) = &media.native {
            if !native.readback(&graph, id) {
                return Err("generated native media bindings differ from the verified import".into());
            }
            continue;
        }
        if let Profile::MonoAtom { frames, bits, start, package, material } = &media.profile {
            let stream_id = format!("{id}/audio/1");
            let item = graph.items.iter().find(|item| item.id == *id)
                .ok_or("generated mono MXF item absent")?;
            let stream = graph.media.iter().find(|stream| stream.id == stream_id)
                .ok_or("generated mono MXF stream absent")?;
            let saved = graph.sidecar("resolve.source", &stream_id)
                .ok_or("generated mono MXF file receipt absent")?;
            if item.umid.as_deref() != Some(&hex::encode_upper(material))
                || item.media.len() != 1 || item.media[0].media != stream_id
                || stream.locations.as_slice() != [utf8_path(&media.path)?]
                || stream.sample_rate != Some(48000) || stream.bits != Some(*bits)
                || stream.start != Rational::new(i128::from(*start) * 1001, 24000)
                || Some(stream.duration) != Rational::new(i128::from(*frames) * 1001, 24000)
                || saved.get("saved_file_package") != Some(&Value::Str(hex::encode_upper(package)))
                || saved.get("saved_selector") != Some(&Value::Int(0))
                || saved.get("saved_channel") != Some(&Value::Int(0))
                || saved.get("saved_cache_track") != Some(&Value::Int(0))
            {
                return Err("generated Resolve mono MXF identity, clock or selector differs from preparation".into());
            }
        }
        if let Some(proxy) = &media.proxy {
            let saved = graph.sidecar("resolve.proxy",id).ok_or("generated proxy absent")?;
            if saved.get("path") != Some(&Value::Str(utf8_path(&proxy.path)?.into()))
                || saved.get("frames") != Some(&Value::Int(proxy.frames))
                || saved.get("start") != Some(&Value::Int(proxy.start))
                || saved.get("rate") != Some(&Value::Str(proxy.rate.to_string()))
                || saved.get("audio_bits") != Some(&proxy.bits.map(Value::Int).unwrap_or(Value::Null))
            {
                return Err("generated Resolve proxy locator, extent or PCM assignment disagrees".into());
            }
        }
        if let (Profile::Mov { rate, frames, .. }, Some(start)) =
            (&media.profile, media.profile.movie_timecode())
        {
            let file = utf8_path(&media.path)?;
            let source = graph.media.iter().find(|source| source.id == format!("{id}/video"))
                .ok_or("generated camera-original video stream absent")?;
            if source.locations.as_slice() != [file]
                || source.codec.as_deref() != Some("apch")
                || source.rate != Some(*rate)
                || Some(source.duration) != Rational::new(i128::from(*frames) * i128::from(rate.den()), rate.num().into())
                || source.start != Rational::new(i128::from(start) * i128::from(rate.den()), rate.num().into())
                || source.clock.as_ref().is_none_or(|clock| clock.effective_count != start as u64)
            {
                return Err("generated camera-original locator, source timecode or extent differs from preparation".into());
            }
        }
        if let Profile::Mov {
            video,
            rate,
            frames,
            raster: (width, height),
            aac,
            pcm,
            ..
        } = &media.profile
        {
            if *video == MovieVideo::ProResHq && pcm.is_none() { continue; }
            let item = graph
                .items
                .iter()
                .find(|item| item.id == *id)
                .ok_or("generated MOV or MP4 media item absent")?;
            let duration = Rational::new(
                i128::from(*frames) * i128::from(rate.den()),
                rate.num().into(),
            )
            .ok_or("generated MOV or MP4 duration overflow")?;
            let file = utf8_path(&media.path)?;
            let video_codec = video.cache_codec()?;
            let audio_codec = if *aac { Some("aac") } else {
                match pcm {
                    Some(_) if *video == MovieVideo::Avc => Some("pcm"),
                    Some(16) => Some("pcm_s16le"),
                    Some(24) => Some("pcm_s24le"),
                    _ => None,
                }
            };
            let expected = [Some(video_codec),audio_codec,audio_codec];
            let mut streams = item.media.iter();
            for codec in expected.into_iter().flatten() {
                let saved = streams
                    .next()
                    .and_then(|link| graph.media.iter().find(|row| row.id == link.media))
                    .ok_or("generated MOV or MP4 media stream absent")?;
                let stream_duration = if matches!(codec, "aac" | "pcm") {
                    Rational::new(movie_audio_sample_extent(*frames, *rate)?.into(), 48_000)
                        .ok_or("generated embedded movie audio duration overflow")?
                } else { duration };
                if saved.locations.as_slice() != [file]
                    || saved.duration != stream_duration
                    || saved.codec.as_deref() != Some(codec)
                    || (codec == "pcm" && saved.bits != *pcm)
                    || (saved.kind == graph::StreamKind::Video
                        && saved.frame_size != Some((i64::from(*width), i64::from(*height))))
                {
                    return Err(
                        "generated MOV or MP4 source clock or locator differs from preparation".into(),
                    );
                }
            }
            if streams.next().is_some() {
                return Err("generated MOV or MP4 media has an unexpected stream".into());
            }
        }
        let Profile::BoundMxf(binding) = &media.profile else {
            continue;
        };
        let Some(files) = &binding.atom else {
            // Reuse the native media check with the IDs allocated by this archive.
            let saved_binding = crate::VerifiedBinding {
                item_id: id.clone(),
                path: binding.path.clone(),
                streams: std::array::from_fn(|index| crate::StreamSelector {
                    media_id: if index == 0 { format!("{id}/video") } else { format!("{id}/audio/{index}") },
                    index,
                    channel: (index > 0).then_some(0),
                }),
                frames: binding.frames,
                timecode_frames: binding.timecode_frames,
                source_package: binding.source_package,
                modified_micros: binding.modified_micros,
                tagged_rec709: binding.tagged_rec709,
                digest: binding.digest,
                atom: None,
            };
            crate::prepare::check_binding_profile(&graph, &saved_binding, true)
                .map_err(|error| format!("generated OP1a camera differs from preparation: {error}"))?;
            continue;
        };
        let item = graph
            .items
            .iter()
            .find(|item| item.id == *id)
            .ok_or("generated split MXF item missing")?;
        if item.umid.as_deref() != Some(&hex::encode_upper(files.master_package))
            || item.media.len() != files.files.len()
        {
            return Err("generated split MXF master identity differs from preparation".into());
        }
        let start = Rational::new(i128::from(binding.timecode_frames) * 1001, 24000);
        for (index, file) in files.files.iter().enumerate() {
            let stream_id = if index == 0 {
                format!("{id}/video")
            } else {
                format!("{id}/audio/{index}")
            };
            let stream = graph.media.iter().find(|row| row.id == stream_id);
            let receipt = graph.sidecar("resolve.source", &stream_id);
            let file_path = utf8_path(&file.path)?;
            if !item.media.iter().any(|link| link.media == stream_id)
                || !stream.is_some_and(|row| {
                    row.locations.as_slice() == [file_path] && row.start == start
                })
                || receipt.and_then(|fields| fields.get("saved_file_package"))
                    != Some(&Value::Str(hex::encode_upper(file.package)))
            {
                return Err("generated split MXF file identity differs from preparation".into());
            }
        }
    }
    let mut recovered_audio = std::collections::HashMap::new();
    let mut recovered_labels = HashSet::new();
    for (index, (timeline, &read)) in p.timelines.iter().zip(&readbacks).enumerate() {
        let count = if let Some(group) = &timeline.group {
            group.prepared.lanes.iter().map(Vec::len).sum::<usize>()
        } else {
            timeline.lanes.iter().map(Vec::len).sum::<usize>()
        };
        let marker_count = timeline.markers.len()
            + timeline.lanes.iter().flatten().map(|edit| edit.markers.len()).sum::<usize>();
        let owner = graph.sidecar("resolve.source", &read.id).ok_or("generated Resolve sequence owner is absent")?;
        let folder = owner.get("bin_id");
        if timeline.compound && (crate::nests::string(owner,"kind") != Some("compound")
            || owner.get("container_ref_agrees") != Some(&Value::Bool(true))
            || crate::nests::string(owner,"media_pool_clip") != ids.timeline_pools[index].as_deref())
        {
            return Err("generated Resolve compound ownership or container reference differs".into());
        }
        if read.id != ids.sequences[index]
            || read.name != timeline.name
            || read.rate != p.rate
            || read.frame_size != Some((i64::from(p.canvas.0), i64::from(p.canvas.1)))
            || read.clock.as_ref().map(|clock| (clock.effective_count, clock.drop))
                != Some((timeline.origin as u64, is_2997(p.rate)))
            || owner.get("record_length") != Some(&Value::Int(timeline.length))
            || read.placements.len() != count
            || crate::prepare::prepare_markers(&graph, read, timeline.length)?.0 != timeline.markers
            || read.markers.len() != marker_count
            || crate::timeline_marks::TimelineMarks::prepare(&graph, &read.id, timeline.length)?
                != timeline.marks
            || crate::track_state::TrackState::prepare(&graph, read)? != timeline.track_state
            || read.texts.len() != timeline.captions.len()
            || crate::generator::prepare(&graph, read)? != timeline.solid
            || !matches!(folder, Some(Value::Str(id)) if *id == ids.bins[timeline.bin])
        {
            return Err(format!(
                "generated Resolve timeline disagrees with {}",
                timeline.name
            )
            .into());
        }
        for (saved, expected) in read.texts.iter().zip(&timeline.captions) {
            let frame = |time| crate::prepare::frames_on(time, p.rate);
            if saved.track != 1
                || saved.kind != graph::source::TextKind::Caption
                || saved.reason.is_some()
                || saved.content.as_deref() != Some(expected.text.as_str())
                || saved.record_start.map(frame).transpose()? != Some(expected.start)
                || saved.duration.map(frame).transpose()? != Some(expected.length)
            {
                return Err("generated Resolve subtitle differs from preparation".into());
            }
        }
        if DirectMixer::prepare(&graph, read)? != timeline.mixer {
            return Err(
                format!("generated Fairlight mixer disagrees with {}", timeline.name).into(),
            );
        }
        if crate::linked::mixer::StaticFaders::prepare(&graph, read)? != timeline.faders {
            return Err(format!("generated Fairlight faders disagree with {}", timeline.name).into());
        }
        if let Some(group) = &timeline.group {
            let id = ids
                .group
                .as_deref()
                .ok_or("generated Group identity absent")?;
            if graph.multicams.len() != 1
                || graph.multicams[0].id != id
                || graph.multicams[0].name != group.prepared.group_name
            {
                return Err("generated Group definition differs from preparation".into());
            }
            let uses: Vec<_> = graph
                .multicam_uses
                .iter()
                .filter(|u| u.sequence == read.id)
                .collect();
            if uses.len() != count {
                return Err("generated Group use count differs from preparation".into());
            }
            for (lane, edits) in group.prepared.lanes.iter().enumerate() {
                let kind = if lane == 0 {
                    graph::StreamKind::Video
                } else {
                    graph::StreamKind::Audio
                };
                let track = lane.max(1) as i64;
                let native = uses.iter().filter(|u| u.kind == kind && u.track == track);
                for (saved, wanted) in native.zip(edits) {
                    let frame = |time| crate::prepare::frames_on(time, p.rate);
                    let [placement] = saved.binding.placements.as_slice() else {
                        return Err("generated Group use lacks one leaf placement".into());
                    };
                    let leaf = read
                        .placements
                        .iter()
                        .find(|p| &p.id == placement)
                        .ok_or("generated Group leaf placement absent")?;
                    if saved.candidates.as_slice() != [id]
                        || saved.selected != Some(wanted.camera as i64)
                        || saved.source_in.map(frame).transpose()? != Some(wanted.edit.source_in)
                        || frame(leaf.record_start)? != wanted.edit.start
                        || frame(leaf.source_in)?
                            != wanted.edit.source_in - group.prepared.offsets[wanted.camera]
                        || frame(leaf.duration)? != wanted.edit.length
                        || leaf.item != ids.media[group.cameras[wanted.camera]]
                        || (lane != 0 && leaf.audio_streams.as_slice() != [lane as i64])
                    {
                        return Err(
                            "generated Group angle, range or channel differs from preparation"
                                .into(),
                        );
                    }
                }
                if uses
                    .iter()
                    .filter(|u| u.kind == kind && u.track == track)
                    .count()
                    != edits.len()
                {
                    return Err("generated Group lane count differs from preparation".into());
                }
            }
            continue;
        }
        for (lane, expected) in timeline.lanes.iter().enumerate() {
            let on_lane = |edit: &&graph::Placement| match lane {
                0 => edit.video_track == Some(1),
                1 if timeline.stereo => crate::stereo::placement(edit),
                3 => edit.video_track == Some(2),
                4 => edit.audio_tracks.as_slice() == [3],
                audio => edit.audio_tracks.as_slice() == [audio as i64],
            };
            for (edit, wanted) in read.placements.iter().filter(on_lane).zip(expected) {
                let saved_markers = graph.sidecar(crate::source::EDIT_MARKER_ADAPTER, &edit.id);
                if saved_markers.is_some() || !wanted.markers.is_empty() {
                    let (actual, omitted) =
                        crate::prepare::prepare_edit_markers(read, edit, saved_markers, None)?;
                    if saved_markers.is_none() || actual != wanted.markers || !omitted.is_empty() {
                        return Err("generated Resolve edit markers differ from preparation".into());
                    }
                }
                if let Some(loss) = wanted.envelope.and_then(crate::audio::Envelope::recovered_loss) {
                    recovered_audio.insert(edit.id.as_str(), loss);
                }
                if wanted.label.is_some() {
                    recovered_labels.insert(edit.id.as_str());
                }
                let frame = |time| crate::prepare::frames_on(time, p.rate);
                let matches_timing =
                    wanted.timing.matches_readback(&graph, edit, wanted.source_in, wanted.length)?;
                if edit.item != ids.media[wanted.media]
                    || frame(edit.record_start)? != wanted.start
                    || frame(edit.source_in)? != wanted.source_in
                    || frame(edit.duration)? != wanted.length
                    || !matches_timing
                    || crate::grade::prepare(&graph, &edit.id)?.map(|grade| grade.0)
                        != wanted.grade.as_ref().map(|grade| grade.0)
                    || !crate::video::matches_readback(edit, wanted.video.as_ref())
                    || !crate::input_scaling::matches_readback(&graph, &read.id, &edit.id, wanted.input_sizing)?
                    || crate::clip_label::ClipLabel::prepare(&graph, &edit.id)? != wanted.label
                    || (matches!(lane, 1 | 2 | 4)
                        && crate::audio::prepare(
                            edit, wanted.source_in, wanted.length, p.rate,
                            graph.sidecar(crate::source::ADAPTER, &edit.id),
                        )? != wanted.envelope)
                    || !edit
                        .audio_streams
                        .iter()
                        .copied()
                        .eq(wanted.channel.map(|index| index as i64 + 1))
                {
                    return Err("generated Resolve edit differs from preparation".into());
                }
            }
            if read.placements.iter().filter(on_lane).count() != expected.len() {
                return Err("generated Resolve track count differs from preparation".into());
            }
        }
    }
    if graph.losses.iter().any(|loss| {
        if recovered_labels.contains(loss.object.as_str()) && crate::clip_label::ClipLabel::recover(loss) {
            return false;
        }
        if recovered_audio
            .get(loss.object.as_str())
            .is_some_and(|(aspect, reason)| loss.aspect == *aspect && loss.reason == *reason)
        {
            return false;
        }
        if loss.aspect == "placement" && loss.reason.split_once(": ").is_some_and(|(id, reason)|
            reason == crate::nests::LOSS && nested_ids.contains(&(loss.object.as_str(), id)))
        {
            return false;
        }
        !p.timelines
            .iter()
            .zip(&readbacks)
            .zip(&dissolves)
            .any(|((timeline, read), dissolve)| {
                timeline.mixer.is_some() && DirectMixer::recover(loss, &read.id)
                    || timeline.faders.recover(loss, &read.id)
                    || timeline.track_state.is_some() && crate::track_state::TrackState::recover(loss, &read.id)
                    || timeline.marks.is_some() && crate::timeline_marks::TimelineMarks::recover(loss, &read.id)
                    || dissolve.iter().any(|d| d.prepared.recover(loss, &read.id))
                    || crate::nests::recover_empty_stereo(&graph, loss, &read.id)
            })
    })
    {
        return Err(format!(
            "generated Resolve reference graph differs from prepared project: {:?}",
            graph.losses
        )
        .into());
    }
    Ok(())
}
