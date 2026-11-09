//! Synthetic content/timing bridge checks; native acceptance is recorded separately.
use prproj::convert::{model::*, prepared::*, writer::prepare_typed_project};
use prproj::{model::Project, SeededUuids};
use std::path::PathBuf;
fn input() -> PreparedProjectInput {
    let frame = TICKS_PER_SECOND / 24;
    PreparedProjectInput {
        video_dissolves: Vec::new(),
        picture_effects: Vec::new(),
        project_name: "Synthetic caption and title".into(),
        options: Options::default(),
        clips: vec![],
        bins: vec![],
        multicams: vec![],
        multicam_uses: vec![],
        sequences: vec![Sequence {
            name: "Caption and Text Control".into(),
            rate: Fraction::new(24, 1),
            width: 1920,
            height: 1080,
            events: vec![],
            markers: vec![],
            track_levels: vec![],
            audio_crossfades: vec![],
            start: 3600 * TICKS_PER_SECOND,
        }],
        texts: vec![
            PreparedText {
                sequence: 0,
                source_id: "synthetic/caption/1".into(),
                kind: PreparedTextKind::Caption,
                track: 1,
                start: 24 * frame,
                duration: 25 * frame,
                content: "Caption Alpha".into(),
            },
            PreparedText {
                sequence: 0,
                source_id: "synthetic/caption/2".into(),
                kind: PreparedTextKind::Caption,
                track: 2,
                start: 60 * frame,
                duration: 24 * frame,
                content: "Café — second line\nCaption Beta".into(),
            },
            PreparedText {
                sequence: 0,
                source_id: "synthetic/title/1".into(),
                kind: PreparedTextKind::Graphic,
                track: 4,
                start: 0,
                duration: 96 * frame,
                content: "Editable Avid Text\nUnicode café".into(),
            },
        ],
    }
}
#[test]
fn embedded_captions_and_graphics_preserve_exact_frames_without_staging_files() {
    let m = input();
    let directory = std::env::var_os("ANYNLE_TEXT_NATIVE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "anynle-text-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ))
        });
    std::fs::create_dir_all(&directory).unwrap();
    let out = directory.join("Caption and Text Control.prproj");
    let seed =
        std::env::var("ANYNLE_TEXT_NATIVE_SEED").unwrap_or_else(|_| "text-native-control".into());
    let mut ids = SeededUuids::new(&seed);
    let prepared = prepare_typed_project(&m, &out, &mut ids, &[], false).unwrap();
    assert!(!out.exists());
    assert_eq!(
        std::fs::read_dir(&directory).unwrap().count(),
        0,
        "text preparation must not create SRT/assets or a project"
    );
    prepared.publish().unwrap();
    assert_eq!(
        std::fs::read_dir(&directory).unwrap().count(),
        1,
        "text stays embedded, with no sidecar caption file"
    );
    let p = Project::from_document(
        prproj::PremiereDocument::from_bytes(&std::fs::read(&out).unwrap()).unwrap(),
        &out,
    )
    .unwrap();
    let seq = p.sequences()[0];
    assert_eq!(p.video_tracks(seq).len(), 4);
    let tracks = p.caption_tracks(seq);
    assert_eq!(tracks.len(), 1);
    let captions = p.captions(tracks[0]);
    assert_eq!(captions.len(), 2);
    for (i, c) in captions.into_iter().enumerate() {
        assert_eq!(p.caption_text(c).replace('\r', "\n"), m.texts[i].content);
        assert_eq!(p.caption_start(c).unwrap().ticks, m.texts[i].start);
        assert_eq!(
            p.caption_end(c).unwrap().ticks,
            m.texts[i].start + m.texts[i].duration
        );
    }
    let graphic = p.track(p.video_tracks(seq)[3]).clips[0];
    assert_eq!(p.start(graphic).unwrap().ticks, 0);
    assert_eq!(p.end(graphic).unwrap().ticks, 4 * TICKS_PER_SECOND);
    let payloads = p.doc.index_payloads().unwrap();
    assert!(
        payloads.values().all(|b| !b
            .windows(b"LucidaConsole".len())
            .any(|w| w == b"LucidaConsole")),
        "new translation must not retain the missing template font"
    );
    let graphic_text = p
        .doc
        .tree
        .children(p.doc.root)
        .iter()
        .copied()
        .find(|&n| {
            p.doc.tree.tag(n) == "ArbVideoComponentParam"
                && p.doc.tree.findtext(n, "Name").unwrap().as_deref() == Some("Source Text")
        })
        .unwrap();
    let value = p
        .doc
        .tree
        .child_by_tag(graphic_text, "StartKeyframeValue")
        .unwrap();
    let hash = p.doc.tree.get(value, "BinaryHash").unwrap();
    let bytes = &payloads[hash];
    assert_eq!(
        prproj::caption::flat::decode_caption_text(bytes)
            .unwrap()
            .replace('\r', "\n"),
        m.texts[2].content
    );
    assert_eq!(
        prproj::caption::flat::read_font_family(bytes).unwrap(),
        "ArialMT"
    );
    assert_eq!(prproj::caption::flat::read_font_size(bytes).unwrap(), 48.0);
}
#[test]
fn typed_text_rejects_unknown_sequence_subframes_overflow_and_controls() {
    let m = input();
    m.validate().unwrap();
    for change in 0..6 {
        let mut m = m.clone();
        match change {
            0 => m.texts[0].sequence = 1,
            1 => m.texts[0].start += 1,
            2 => m.texts[0].duration = i64::MAX,
            3 => m.texts[0].content = "bad\0text".into(),
            4 => m.texts[0].track = 65,
            _ => m.sequences[0].rate.num = 0,
        }
        assert!(m.validate().is_err(), "control {change}");
    }
}
