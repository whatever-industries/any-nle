//! Small synthetic packages exercise graph validation without proprietary fixtures.
use super::*;
use crate::binary::{qt as encode_qt, Qt};
use std::{fs::File, io::Write};
use zip::{write::SimpleFileOptions, ZipWriter};

const PROJECT: &str = "11111111-0000-0000-0000-000000000000";
const POOL: &str = "22222222-0000-0000-0000-000000000000";
const ROOT: &str = "33333333-0000-0000-0000-000000000000";
const CHILD: &str = "44444444-0000-0000-0000-000000000000";
const CLIP: &str = "55555555-0000-0000-0000-000000000000";
const HANDLE: &str = "66666666-0000-0000-0000-000000000000";
const SEQUENCE: &str = "77777777-0000-0000-0000-000000000000";

#[test]
fn imported_movie_split_selectors_follow_physical_channel_order() {
    let selector = |channel| {
        encode_qt(&[
            ("AudioType", Qt::Int(1)),
            ("ChannelsBA", Qt::Bytes(&[0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, channel])),
        ])
    };
    let left = selector(1);
    let right = selector(2);
    let clip = |second: &[u8]| {
        let routes = hex::encode(encode_qt(&[("0", Qt::Bytes(&left)), ("1", Qt::Bytes(second))]));
        xml::parse(
            &format!("<Sm2MpVideoClip><VirtualAudioTracksBA>{routes}</VirtualAudioTracksBA></Sm2MpVideoClip>"),
            &mut 0,
        ).unwrap()
    };
    let mut blobs = Blobs::default();
    assert_eq!(
        native_mono_selectors(&clip(&right), &mut blobs).unwrap(),
        Some([hex::encode(&left), hex::encode(&right)]),
    );
    assert!(native_mono_selectors(&clip(&left), &mut blobs).unwrap().is_none());
}

#[test]
fn embedded_movie_overrides_physical_audio_channel_without_changing_track() {
    let make = |stream, channel| {
        let selector = hex::encode(encode_qt(&[
            ("AudioType", Qt::Int(1)),
            ("ChannelsBA", Qt::Bytes(&[0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x40, channel])),
        ]));
        xml::parse(
            &format!("<Edit><VirtualAudioTrackBA>{selector}</VirtualAudioTrackBA><MediaTrackIdx>{stream}</MediaTrackIdx></Edit>"),
            &mut 0,
        ).unwrap()
    };
    let mut blobs = Blobs::default();
    assert_eq!(mono(&make(0, 2), &mut blobs, true, true).unwrap(), Some(1));
    assert_eq!(mono(&make(1, 2), &mut blobs, true, true).unwrap(), Some(1));
    assert!(mono(&make(1, 1), &mut blobs, true, true).is_err());
    assert!(mono(&make(0, 2), &mut blobs, true, false).is_err());
}

fn build(path: &Path, handle: &str, child_parent: &str) {
    build_with_child_registration(path, handle, child_parent, CHILD);
}

#[test]
fn embedded_aac_keeps_44100_sample_clock_and_saved_channel_route() {
    const CACHE: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const EDIT: &str = "bbbbbbbb-0000-0000-0000-000000000001";
    const TRACK: &str = "cccccccc-0000-0000-0000-000000000001";
    const SAMPLES: i64 = 441037;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("aac-source.drp");
    build(&path, HANDLE, ROOT);
    let origin = 128.0 / 44100.0;
    let track = encode_qt(&[
        ("NumChannels", Qt::Int(2)), ("IdxTrack", Qt::Int(0)),
        ("SampleRate", Qt::Int(44100)), ("BitDepth", Qt::Int(3)),
        ("Duration", Qt::Long(SAMPLES)), ("StartTime", Qt::Double(origin)),
        ("ChannelLayout", Qt::Int(2)), ("CodecName", Qt::Text("AAC")),
    ]);
    let tracks = hex::encode(encode_qt(&[("0", Qt::Bytes(&track))]));
    let cache = xml::parse(&format!(r#"<BtAudioInfo DbId="{CACHE}"><TracksBA>{tracks}</TracksBA></BtAudioInfo>"#), &mut 0).unwrap();
    let clip = |saved_origin: f64, right: i32| {
        let channel = |index| encode_qt(&[("MediaRef", Qt::Text(CACHE)), ("ChannelIdx", Qt::Int(index))]);
        let left = channel(0);
        let right = channel(right);
        let mapping = encode_qt(&[("0", Qt::Bytes(&left)), ("1", Qt::Bytes(&right))]);
        let mut extent = saved_origin.to_le_bytes().to_vec();
        extent.extend_from_slice(&(SAMPLES as f64 / 44100.0).to_le_bytes());
        let route = encode_qt(&[
            ("Type", Qt::Int(0)), ("CodecName", Qt::Text("AAC")),
            ("MediaExtents", Qt::Bytes(&extent)), ("ChannelVecBA", Qt::Bytes(&mapping)),
        ]);
        let routes = encode_qt(&[("0", Qt::Bytes(&route))]);
        let mut bytes = Vec::new();
        crate::binary::pb_bytes(5, &routes, &mut bytes);
        for _ in 0..2 {
            let inner = std::mem::take(&mut bytes);
            crate::binary::pb_bytes(1, &inner, &mut bytes);
        }
        let fields = crate::binary::pack(&bytes).unwrap();
        xml::parse(&format!(r#"<Sm2MpVideoClip DbId="{CLIP}"><FieldsBlob>{fields}</FieldsBlob></Sm2MpVideoClip>"#), &mut 0).unwrap()
    };
    let capture = |clip: &Node| {
        let mut graph = read(&path).unwrap();
        let mut item = SourceItem {
            id: CLIP.into(), name: "Stereo clip".into(), media: vec![],
            umid: None, tape: None, rate: None, color_rgb16: None, stream_groups: vec![], markers: vec![],
        };
        assert!(graph.media.is_empty());
        let saved = read_embedded_stereo(clip, &cache, "/fixture/stereo.mp4", 234, Rational::integer(24),
            86400, "AAC", None, &mut Blobs::default(), &mut graph, &mut item)?;
        assert!(saved);
        graph.items.push(item);
        Ok::<_, Box<dyn std::error::Error>>(graph)
    };
    assert!(capture(&clip(129.0 / 44100.0, 1)).is_err());
    assert!(capture(&clip(origin, 0)).is_err());
    let mut graph = capture(&clip(origin, 1)).unwrap();
    assert_eq!(graph.items[0].stream_groups, vec![vec![1, 2]]);
    assert!(graph.items[0].media.iter().all(|stream| stream.source_offset.is_none()));
    for (index, media) in graph.media.iter().enumerate() {
        assert_eq!(media.sample_rate, Some(44100));
        assert_eq!(media.duration, Rational::from_units(SAMPLES, 44100).unwrap());
        assert_eq!(media.start, Rational::from_units(128, 44100));
        assert_eq!(media.stream, Some(index as i64 + 1));
        assert_eq!(graph.sidecar(ADAPTER, &media.id).unwrap().get("saved_channel"), Some(&Value::Int(index as i64)));
        assert!(media.original.is_none());
    }
    let selector = crate::stereo::SELECTOR;
    let edit = xml::parse(&format!(r#"<Sm2TiAudioClip DbId="{EDIT}"><MediaRef>{CLIP}</MediaRef>
        <MediaTrackIdx>0</MediaTrackIdx><VirtualAudioTrackBA>{selector}</VirtualAudioTrackBA>
        <Start>86417</Start><Duration>24</Duration><In>9|000000000000e03f</In>
        <MediaTimemapBA>023ff00000000000000000000000000000000000000000000000000000000000000000000000000000</MediaTimemapBA>
        <MediaStartTime>0</MediaStartTime><Flags>0</Flags></Sm2TiAudioClip>"#), &mut 0).unwrap();
    let track = xml::parse(&format!(r#"<Sm2TiTrack DbId="{TRACK}"><SubType>1</SubType><Flags>0</Flags></Sm2TiTrack>"#), &mut 0).unwrap();
    let sequence = graph.sequences[0].clone();
    embedded_audio::observe_edit(Rational::integer(24), &edit, &track, 2, &sequence, &mut graph, &mut Blobs::default()).unwrap();
    let Some(Value::Arr(routes)) = graph.sidecar(embedded_audio::ROUTE_ADAPTER, SEQUENCE) else { panic!("saved AAC route") };
    assert_eq!(routes[0].get("physical_channels"), Some(&Value::Arr(vec![Value::Int(1), Value::Int(2)])));
    assert_eq!(routes[0].get("saved_track_subtype"), Some(&text("1")));
    let Some(Value::Arr(timing)) = graph.sidecar("resolve.timing", SEQUENCE) else { panic!("saved AAC timing") };
    assert_eq!(timing[0].get("saved_in"), Some(&text("9|000000000000e03f")));
    assert!(graph.sequences[0].placements.is_empty());
    assert!(graph.losses.iter().any(|row| row.reason.starts_with(EDIT)));
    assert_eq!(SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap(), graph);
}

#[test]
fn saved_still_and_stereo_mp3_round_trip_without_physical_media_claims() {
    const STILL: &str = "88888888-0000-0000-0000-000000000000";
    const AUDIO: &str = "99999999-0000-0000-0000-000000000000";
    const CACHE: &str = "aaaaaaaa-0000-0000-0000-000000000000";
    const STILL_EDIT: &str = "bbbbbbbb-0000-0000-0000-000000000000";
    const AUDIO_EDIT: &str = "cccccccc-0000-0000-0000-000000000000";
    const RATE: &str = "00000000000038400000000000000000";
    const RASTER: &str = "00000000000007800000000000000438";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("media-sources.drp");
    build(&path, HANDLE, ROOT);
    let mut graph = read(&path).unwrap();
    let mut blobs = Blobs::default();
    let cache = |name: &str, codec: Option<&str>| {
        let mut fields = Vec::new();
        crate::binary::pb_bytes(1, b"/media", &mut fields);
        crate::binary::pb_bytes(2, name.as_bytes(), &mut fields);
        if let Some(codec) = codec {
            crate::binary::pb_bytes(5, codec.as_bytes(), &mut fields);
        }
        crate::binary::pb_bytes(6, name.as_bytes(), &mut fields);
        crate::binary::pack(&fields).unwrap()
    };
    let timing = hex::encode(encode_qt(&[
        ("FrameRate", Qt::Bytes(&hex::decode(RATE).unwrap())),
        ("NumFrames", Qt::Int(1)), ("StartFrame", Qt::Int(301)),
    ]));
    let mut geometry = encode_qt(&[
        ("Resolution", Qt::Bytes(&hex::decode(RASTER).unwrap())),
        ("PixelAspectRatio", Qt::Double(1.125)),
    ]);
    let type_start = geometry.len() - 13;
    geometry[type_start..type_start + 4].copy_from_slice(&135u32.to_be_bytes());
    let geometry = hex::encode(geometry);
    let still_node = |name| xml::parse(&format!(
        r#"<Sm2MpVideoClip DbId="{STILL}"><Name>Image</Name><Video><BtVideoInfo><Type>STANDARD_CLIP</Type><Clip>{}</Clip><Time>{timing}</Time><Geometry>{geometry}</Geometry></BtVideoInfo></Video></Sm2MpVideoClip>"#,
        cache(name, None),
    ), &mut 0).unwrap();
    let caches = HashMap::new();
    assert!(media(&still_node("still[301-305].png"), &caches, &mut blobs, &mut graph).unwrap().is_none());
    let image = media(&still_node("still0301.png"), &caches, &mut blobs, &mut graph).unwrap().unwrap();
    assert_eq!(graph.sidecar(ADAPTER, STILL).unwrap().get("saved_cache_start_frame"), Some(&Value::Int(301)));
    let image_stream = &graph.media[0].id;
    let pixel_aspect = graph.sidecar("resolve.media_geometry", image_stream).unwrap();
    assert_eq!(pixel_aspect.get("pixel_aspect_ratio"), Some(&text("9/8")));
    assert_eq!(pixel_aspect.get("saved_pixel_aspect_bits"), Some(&text("3ff2000000000000")));
    assert!(graph.losses.iter().any(|loss| loss.object == *image_stream && loss.aspect == "geometry"));

    let channel = |index| encode_qt(&[("MediaRef", Qt::Text(CACHE)), ("ChannelIdx", Qt::Int(index))]);
    let left = channel(0);
    let right = channel(1);
    let mapping = encode_qt(&[("0", Qt::Bytes(&left)), ("1", Qt::Bytes(&right))]);
    let mut media_extent = vec![0; 8];
    media_extent.extend_from_slice(&10.0f64.to_le_bytes());
    let route = encode_qt(&[
        ("Type", Qt::Int(0)), ("CodecName", Qt::Text("MP3")),
        ("MediaExtents", Qt::Bytes(&media_extent)), ("ChannelVecBA", Qt::Bytes(&mapping)),
    ]);
    let routes = encode_qt(&[("0", Qt::Bytes(&route))]);
    let mut route_fields = Vec::new();
    crate::binary::pb_bytes(5, &routes, &mut route_fields);
    let mut fields = Vec::new();
    crate::binary::pb_bytes(1, &route_fields, &mut fields);
    let mut import_clock = vec![9];
    import_clock.extend_from_slice(&24.0f64.to_le_bytes());
    crate::binary::pb_bytes(2, &import_clock, &mut fields);
    let fields = crate::binary::pack(&fields).unwrap();
    let track = encode_qt(&[
        ("SampleRate", Qt::Int(44100)), ("NumChannels", Qt::Int(2)), ("BitDepth", Qt::Int(3)),
        ("Duration", Qt::Int(441000)), ("StartTime", Qt::Double(0.0)),
        ("CodecName", Qt::Text("MP3")), ("IdxTrack", Qt::Int(0)),
    ]);
    let tracks = hex::encode(encode_qt(&[("0", Qt::Bytes(&track))]));
    let mp3 = xml::parse(&format!(
        r#"<Sm2MpAudioClip DbId="{AUDIO}"><Name>Music</Name><FieldsBlob>{fields}</FieldsBlob><EmbeddedAudioVec><Element><BtAudioInfo DbId="{CACHE}"><Clip>{}</Clip><TracksBA>{tracks}</TracksBA></BtAudioInfo></Element></EmbeddedAudioVec></Sm2MpAudioClip>"#,
        cache("music.mp3", Some("MP3")),
    ), &mut 0).unwrap();
    let audio = wav::read(&mp3, &mut blobs, &mut graph).unwrap().unwrap();
    assert_eq!(graph.media[1].sample_rate, Some(44100));
    assert_eq!(graph.media[2].duration, Rational::integer(10));
    assert!(graph.media.iter().all(|media| media.original.is_none() && media.online.is_none()));

    let grid = Grid { origin: 0, rate: Rational::integer(24), drop: false, length: 120, size: (1920,1080) };
    let mut hold_keys = vec![0x80, 0x0a, 0, 0x0a, 9, 9];
    hold_keys.extend_from_slice(&60000.0f64.to_le_bytes());
    let hold = hex::encode(encode_qt(&[
        ("YMin", Qt::Double(-1.0)), ("YMax", Qt::Double(-1.0)), ("XMax", Qt::Double(60000.0)),
        ("UniqueId", Qt::Text(STILL_EDIT)), ("DbType", Qt::Text("Sm2TimeMap")),
        ("KeyframesBA", Qt::Bytes(&hold_keys)),
    ]));
    let node = xml::parse(&format!(
        r#"<Sm2TiVideoClip DbId="{STILL_EDIT}"><MediaRef>{STILL}</MediaRef><Start>17</Start><Duration>24</Duration><In>86437</In><MediaStartTime>0</MediaStartTime><MediaFrameRate>{RATE}</MediaFrameRate><MediaTimemapBA>{hold}</MediaTimemapBA></Sm2TiVideoClip>"#,
    ), &mut 0).unwrap();
    let still_placement = still::edit(&image, &node, grid, 1, SEQUENCE, &mut graph, &mut blobs).unwrap().unwrap();
    assert_eq!(still_placement.frame_map, Some(vec![0; 24]));
    let map = hex::encode((10.0f64 - 1.0 / 24.0).to_be_bytes());
    let audio_edit = |track| xml::parse(&format!(
        r#"<Sm2TiAudioClip DbId="{AUDIO_EDIT}"><MediaRef>{AUDIO}</MediaRef><Start>17</Start><Duration>24</Duration><In>9</In><MediaStartTime>0</MediaStartTime><MediaFrameRate>{RATE}</MediaFrameRate><MediaTimemapBA>02{map}</MediaTimemapBA><MediaTrackIdx>{track}</MediaTrackIdx><VirtualAudioTrackBA>{}</VirtualAudioTrackBA></Sm2TiAudioClip>"#,
        crate::stereo::SELECTOR,
    ), &mut 0).unwrap();
    let audio_placement = wav::edit(&audio, &audio_edit(0), grid, 2, true, SEQUENCE, &mut graph, &mut blobs).unwrap().unwrap();
    assert_eq!(audio_placement.source_in, Rational::new(3, 8).unwrap());
    assert_eq!(audio_placement.grouped_audio[0].track, 2);
    assert_eq!(audio_placement.grouped_audio[0].streams, vec![1, 2]);
    assert!(wav::edit(&audio, &audio_edit(1), grid, 2, true, SEQUENCE, &mut graph, &mut blobs).unwrap().is_none());

    let seq = &mut graph.sequences[0];
    seq.rate = grid.rate;
    seq.start = Some(Rational::ZERO);
    seq.clock = Some(grid.clock(SEQUENCE).unwrap());
    seq.tracks = vec![SourceTrack::new(StreamKind::Video, 1, None), SourceTrack::new(StreamKind::Audio, 2, None)];
    seq.placements = vec![still_placement, audio_placement];
    graph.validate().unwrap();
    assert_eq!(SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap(), graph);
}

#[test]
fn standalone_pcm_wav_captures_physical_channels_without_opening_original() {
    const CACHE: &str = "aaaaaaaa-0000-0000-0000-000000000001";
    const SAMPLES: i64 = 144_017;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wav-source.drp");
    build(&path, HANDLE, ROOT);
    let mut cache = Vec::new();
    crate::binary::pb_bytes(1, b"/unavailable-fixture", &mut cache);
    crate::binary::pb_bytes(2, b"stereo.wav", &mut cache);
    crate::binary::pb_bytes(5, b"Linear PCM", &mut cache);
    let cache = crate::binary::pack(&cache).unwrap();

    for (depth, bits, representation) in [(1, 16, "pcm16"), (2, 24, "pcm24")] {
        let track = encode_qt(&[
            ("SampleRate", Qt::Int(48000)), ("NumChannels", Qt::Int(2)),
            ("BitDepth", Qt::Int(depth)), ("Duration", Qt::Long(SAMPLES)),
            ("StartTime", Qt::Double(0.0)), ("CodecName", Qt::Text("Linear PCM")),
            ("IdxTrack", Qt::Int(0)),
        ]);
        let tracks = hex::encode(encode_qt(&[("0", Qt::Bytes(&track))]));
        let capture = |right| {
            let channel = |index| encode_qt(&[
                ("MediaRef", Qt::Text(CACHE)), ("TrackIdx", Qt::Int(0)),
                ("ChannelIdx", Qt::Int(index)),
            ]);
            let left = channel(0);
            let right = channel(right);
            let mapping = encode_qt(&[("0", Qt::Bytes(&left)), ("1", Qt::Bytes(&right))]);
            let mut extent = vec![0; 8];
            extent.extend_from_slice(&(SAMPLES as f64 / 48000.0).to_le_bytes());
            let route = encode_qt(&[
                ("Type", Qt::Int(0)), ("CodecName", Qt::Text("Linear PCM")),
                ("BitDepth", Qt::Int(depth)), ("MediaExtents", Qt::Bytes(&extent)),
                ("ChannelVecBA", Qt::Bytes(&mapping)),
            ]);
            let routes = encode_qt(&[("0", Qt::Bytes(&route))]);
            let mut pool = Vec::new();
            crate::binary::pb_bytes(5, &routes, &mut pool);
            let mut fields = Vec::new();
            crate::binary::pb_bytes(1, &pool, &mut fields);
            let mut clock = vec![9];
            clock.extend_from_slice(&24.0f64.to_le_bytes());
            crate::binary::pb_bytes(2, &clock, &mut fields);
            let fields = crate::binary::pack(&fields)?;
            let clip = xml::parse(&format!(
                r#"<Sm2MpAudioClip DbId="{CLIP}"><Name>Stereo</Name><FieldsBlob>{fields}</FieldsBlob><EmbeddedAudioVec><Element><BtAudioInfo DbId="{CACHE}"><Clip>{cache}</Clip><TracksBA>{tracks}</TracksBA></BtAudioInfo></Element></EmbeddedAudioVec></Sm2MpAudioClip>"#,
            ), &mut 0)?;
            let mut graph = read(&path)?;
            assert!(wav::read(&clip, &mut Blobs::default(), &mut graph)?.is_some());
            Ok::<_, Box<dyn std::error::Error>>(graph)
        };
        assert!(capture(0).unwrap_err().to_string().contains("ambiguous standalone PCM physical channel"));
        let graph = capture(1).unwrap();
        let reloaded = SourceGraph::from_json_text(&graph.to_json_text().unwrap()).unwrap();
        assert_eq!(graph, reloaded);
        assert_eq!(reloaded.media.len(), 2);
        for (channel, media) in reloaded.media.iter().enumerate() {
            let original = media.original.as_ref().unwrap();
            assert_eq!(media.online, None);
            assert_eq!(media.bits, Some(bits));
            assert_eq!(original.saved_locators, ["/unavailable-fixture/stereo.wav"]);
            assert_eq!(original.container, "wav");
            assert_eq!(original.stream_index, 0);
            assert_eq!(original.channel_index, Some(channel as i64));
            assert_eq!(original.channels, Some(2));
            assert_eq!(original.time_base, Rational::new(1, 48000).unwrap());
            assert_eq!(original.duration_ts, SAMPLES);
            assert_eq!(original.start_pts, 0);
            assert_eq!(original.sample_representation.as_deref(), Some(representation));
            assert_eq!(reloaded.sidecar(ADAPTER, &media.id).unwrap().get("saved_channel"),
                Some(&Value::Int(channel as i64)));
        }
    }
}

fn build_with_child_registration(path: &Path, handle: &str, child_parent: &str, registered: &str) {
    let fields = hex::encode(encode_qt(&[("RootFolderRef", Qt::Text(ROOT))]));
    let mut child_list = Vec::new();
    crate::binary::pb_bytes(
        2,
        &encode_qt(&[("0", Qt::Text(registered))]),
        &mut child_list,
    );
    let folder_fields = crate::binary::pack(&child_list).unwrap();
    let project = format!(
        r#"<SM_Project DbId="{PROJECT}">
        <ProjectVersion>17</ProjectVersion><ProjectName>Fixture &amp; test</ProjectName>
        <TimelineHandleVec><Element>{handle}</Element></TimelineHandleVec>
        <CurrentTimelineIndex>0</CurrentTimelineIndex>
        <MediaPool><Sm2MediaPool DbId="{POOL}"><FieldsBlob>{fields}</FieldsBlob></Sm2MediaPool></MediaPool>
        </SM_Project>"#
    );
    let root = format!(
        r#"<Sm2MpFolder DbId="{ROOT}">
        <FieldsBlob>{folder_fields}</FieldsBlob><Name>Master</Name><MediaPool>{POOL}</MediaPool><MediaVec>
        <Element><Sm2MpTimelineClip DbId="{CLIP}"><MpFolder>{ROOT}</MpFolder>
        <TimelineSharedHandle><Sm2Timeline DbId="{HANDLE}"><Name>Empty test</Name>
        <MpTimelineClip>{CLIP}</MpTimelineClip><Sequence><Sm2Sequence DbId="{SEQUENCE}">
        <Parent>{HANDLE}</Parent><FrameRate>872211b5dcf937400000000000000000</FrameRate>
        <Resolution>00000000000007800000000000000438</Resolution>
        <MediaExtents>333333333327ac400000000000000000</MediaExtents>
        </Sm2Sequence></Sequence></Sm2Timeline></TimelineSharedHandle></Sm2MpTimelineClip></Element>
        </MediaVec></Sm2MpFolder>"#
    );
    let child = format!(
        r#"<Sm2MpFolder DbId="{CHILD}"><Name>Child</Name><MpFolder>{child_parent}</MpFolder>
        <MediaPool>{POOL}</MediaPool><MediaVec/></Sm2MpFolder>"#
    );
    let mut archive = ZipWriter::new(File::create(path).unwrap());
    for (name, contents) in [
        ("project.xml", project),
        (ROOT_FOLDER, root),
        ("MediaPool/Master/000_Child/MpFolder.xml", child),
    ] {
        archive
            .start_file(name, SimpleFileOptions::default())
            .unwrap();
        writeln!(archive, r#"<?xml version="1.0"?>"#).unwrap();
        writeln!(archive, r#"<!--DbAppVer="21.2.0.0006" DbPrjVer="17"-->"#).unwrap();
        archive.write_all(contents.as_bytes()).unwrap();
    }
    archive.finish().unwrap();
}

#[test]
fn saved_project_identity_and_exact_clock_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.drp");
    build(&path, HANDLE, ROOT);
    let graph = read(&path).unwrap();
    assert_eq!(graph.project.name, "Fixture & test");
    assert_eq!(graph.bins.len(), 2);
    assert_eq!(graph.bins[1].parent.as_deref(), Some(ROOT));
    assert_eq!(graph.sequences.len(), 1);
    let timeline = &graph.sequences[0];
    assert_eq!(timeline.id, SEQUENCE);
    assert_eq!(
        graph.sidecar(ACTIVE_TIMELINE_ADAPTER, ROOT),
        Some(&Value::Obj(vec![
            ("saved_index".into(), Value::Int(0)),
            ("sequence_id".into(), text(SEQUENCE)),
            ("unavailable_reason".into(), Value::Null),
        ]))
    );
    assert_eq!(
        graph
            .sidecar(ADAPTER, SEQUENCE)
            .and_then(|row| row.get("bin_id")),
        Some(&Value::Str(ROOT.into()))
    );
    assert_eq!(timeline.frame_size, Some((1920, 1080)));
    assert_eq!(timeline.clock.as_ref().unwrap().effective_count, 86_400);
    assert_eq!(timeline.start, Rational::new(18_018, 5));
    assert!(timeline.placements.is_empty());
    let mut json = Vec::new();
    graph.write_json(&mut json).unwrap();
    assert_eq!(
        SourceGraph::from_json_text(std::str::from_utf8(&json).unwrap()).unwrap(),
        graph
    );
    let mut repeated = Vec::new();
    read(&path).unwrap().write_json(&mut repeated).unwrap();
    assert_eq!(json, repeated);
}

#[test]
fn sibling_order_follows_numeric_keys_in_the_saved_map() {
    let mut nodes = 0;
    let pool_fields = hex::encode(encode_qt(&[("RootFolderRef", Qt::Text(ROOT))]));
    let project = xml::parse(
        &format!(
            r#"<SM_Project><MediaPool><Sm2MediaPool DbId="{POOL}"><FieldsBlob>{pool_fields}</FieldsBlob></Sm2MediaPool></MediaPool></SM_Project>"#
        ),
        &mut nodes,
    )
    .unwrap();
    let mut members = BTreeMap::new();
    let ids: Vec<_> = (100..111).map(|n| uuid::Uuid::from_u128(n).to_string()).collect();
    for (ordinal, id) in ids.iter().enumerate() {
        let path = format!("MediaPool/Master/{ordinal:03}_Duplicate/MpFolder.xml");
        let node = xml::parse(
            &format!(r#"<Sm2MpFolder DbId="{id}"><MediaPool>{POOL}</MediaPool><MpFolder>{ROOT}</MpFolder></Sm2MpFolder>"#),
            &mut nodes,
        )
        .unwrap();
        members.insert(path, node);
    }
    let keys: Vec<_> = (0..ids.len()).map(|ordinal| ordinal.to_string()).collect();
    let rows: Vec<_> = keys
        .iter()
        .zip(ids.iter().rev())
        .map(|(key, id)| (key.as_str(), Qt::Text(id)))
        .collect();
    let mut body = Vec::new();
    crate::binary::pb_bytes(2, &encode_qt(&rows), &mut body);
    let fields = crate::binary::pack(&body).unwrap();
    members.insert(
        ROOT_FOLDER.into(),
        xml::parse(
            &format!(r#"<Sm2MpFolder DbId="{ROOT}"><MediaPool>{POOL}</MediaPool><FieldsBlob>{fields}</FieldsBlob></Sm2MpFolder>"#),
            &mut nodes,
        )
        .unwrap(),
    );

    let ordered = folders(&members, &project, &mut Blobs::default()).unwrap();
    assert_eq!(ordered[0].0.id().unwrap(), ROOT);
    assert!(matches!(
        ordered[0].1,
        Some(BinOrder::Saved { archive_matches: Some(false) })
    ));
    assert!(ordered.iter().skip(1).map(|(node, _)| node.id().unwrap())
        .eq(ids.iter().rev().map(String::as_str)));
}

#[test]
fn broken_registration_and_bin_parent_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("contradictory.drp");
    build(&path, CHILD, ROOT);
    assert!(read(&path)
        .unwrap_err()
        .to_string()
        .contains("registered Resolve timeline"));
    build(&path, HANDLE, CHILD);
    assert!(read(&path).unwrap_err().to_string().contains("bin parent"));
    build_with_child_registration(&path, HANDLE, ROOT, HANDLE);
    assert!(read(&path)
        .unwrap_err()
        .to_string()
        .contains("child bin registration contradicts"));
}

#[test]
fn temporary_archive_keeps_project_validation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("review.drp");
    let read_temporary = || {
        let mut scratch = tempfile::tempfile().unwrap();
        std::io::copy(&mut File::open(&path).unwrap(), &mut scratch).unwrap();
        read_staged(scratch)
    };

    build(&path, HANDLE, ROOT);
    assert_eq!(read_temporary().unwrap(), read(&path).unwrap());

    build_with_child_registration(&path, HANDLE, ROOT, HANDLE);
    let error = read_temporary().unwrap_err().to_string();
    assert!(error.contains("child bin registration contradicts"));
    assert_eq!(error, read(&path).unwrap_err().to_string());
}

#[test]
fn transition_observations_keep_their_owners_after_graph_serialization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("synthetic.drp");
    build(&path, HANDLE, ROOT);
    let mut graph = read(&path).unwrap();
    let mut second = graph.sequences[0].clone();
    second.id = "another-sequence".into();
    second.clock.as_mut().unwrap().provenance.source_package = second.id.clone();
    graph.sequences.push(second);
    let grid = Grid {
        rate: crate::prepare::rate(),
        drop: false,
        origin: 86_400,
        length: 60,
        size: (1920, 1080),
    };
    let make = |id: &str, alignment: &str| Node {
        name: "Sm2TiTransition".into(),
        id: Some(id.into()),
        text: String::new(),
        content: 0..0,
        children: [
            ("Name", "Cross Dissolve"),
            ("Start", "86417"),
            ("Duration", "8"),
            ("AlignmentType", alignment),
            ("Position", "2"),
        ]
        .into_iter()
        .map(|(name, value)| Node {
            name: name.into(),
            id: None,
            text: value.into(),
            content: 0..0,
            children: vec![],
        })
        .collect(),
    };
    for (sequence, track, id, alignment) in [
        (SEQUENCE, "track-a", "transition-a", "3"),
        ("another-sequence", "track-b", "transition-b", "1"),
    ] {
        observations::transition(
            &mut graph, &make(id, alignment), sequence, track, StreamKind::Video, 1, grid, [None; 2], false,
        ).unwrap();
    }
    let mut serialized = Vec::new();
    graph.write_json(&mut serialized).unwrap();
    let reopened = SourceGraph::from_json_text(std::str::from_utf8(&serialized).unwrap()).unwrap();
    for (sequence, alignment) in [(SEQUENCE, "3"), ("another-sequence", "1")] {
        let Some(Value::Arr(rows)) = reopened.sidecar("resolve.transitions", sequence) else {
            panic!("missing transition receipt");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("alignment_type"), Some(&text(alignment)));
        assert_eq!(rows[0].get("position"), Some(&text("2")));
    }
    assert!(reopened.sequences.iter().all(|s| s.transitions.is_empty()));
    assert_eq!(reopened.losses.len(), 2);
}
