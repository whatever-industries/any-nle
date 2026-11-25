//! Bind a static JPEG fill to the hard rectangular mask decoded from its saved PNG alpha.
use super::*;
use avid_adapter::original::{verify_imported_jpeg, verify_imported_jpeg_mxf};
use graph::source::{SourceItem, SourceMedia, SourceSequence};
use prproj::convert::prepared::{
    PreparedJpegFill, PreparedSplitMatte, PreparedText, PreparedTextKind,
};

pub(super) const PROFILE: &str = "split_still_matte";

#[derive(Default)]
pub(super) struct Sources<'a> {
    items: HashMap<&'a str, Option<&'a SourceItem>>,
    media: HashMap<&'a str, &'a SourceMedia>,
    native: HashMap<&'a str, &'a Value>,
    pinned: HashSet<&'a Path>,
    ordinary_sources: HashSet<&'a str>,
    pub(super) saved_imports: HashMap<&'a str, Option<(usize, &'a Value)>>,
    pub(super) imports: HashMap<Vec<String>, Option<SourceInput>>,
    pub(super) managed_inputs: HashMap<&'a Path, (&'a str, u64, SourceInput)>,
    pub(super) used_alpha: HashSet<PathBuf>,
    fills: HashMap<(usize, String), usize>,
}

impl<'a> Sources<'a> {
    pub(super) fn new(read: &'a ReadResult) -> Self {
        let mut items = HashMap::new();
        for item in &read.graph.items {
            if let Some(umid) = item.umid.as_deref() {
                items
                    .entry(umid)
                    .and_modify(|found| *found = None)
                    .or_insert(Some(item));
            }
        }
        Self {
            items,
            media: read
                .graph
                .media
                .iter()
                .map(|m| (m.id.as_str(), m))
                .collect(),
            native: read
                .graph
                .sidecars
                .iter()
                .filter(|sidecar| sidecar.adapter == "avid.source")
                .map(|sidecar| (sidecar.object.as_str(), &sidecar.data))
                .collect(),
            pinned: read
                .original_inputs
                .iter()
                .map(|input| input.path.as_path())
                .collect(),
            ordinary_sources: read.graph.sequences.iter().flat_map(|sequence| {
                sequence.placements.iter().chain(&sequence.disabled_placements)
                    .map(|edit| edit.item.as_str())
            }).chain(read.graph.multicams.iter().flat_map(|group| &group.lanes)
                .flat_map(|lane| &lane.alternatives).flat_map(|angle| &angle.pieces)
                .filter_map(|piece| piece.item.as_deref())).collect(),
            ..Self::default()
        }
    }

    fn linked_jpeg(&self, umid: &str, duration: i64) -> Option<(&'a SourceItem, &'a Path)> {
        let item = self.items.get(umid)?.as_ref()?;
        let [binding] = item.media.as_slice() else {
            return None;
        };
        let media = self.media.get(binding.media.as_str())?;
        let original = media.original.as_ref()?;
        let [path] = media.locations.as_slice() else {
            return None;
        };
        let path = Path::new(path);
        (media.kind == StreamKind::Video
            && media.online == Some(true)
            && media.frame_size == Some((1920, 1080))
            && media.rate == Rational::new(24_000, 1001)
            && binding.source_offset == Some(Rational::integer(0))
            && media
                .duration
                .to_units(TICKS_PER_SECOND)
                .is_some_and(|end| end >= duration)
            && original.container == "jpeg"
            && original.stream_index == 0
            && original.start_pts == 0
            && original.duration_ts == 1
            && self.pinned.contains(path))
        .then_some((item, path))
    }
}

pub(super) fn rows(receipt: &Value) -> impl Iterator<Item = &Value> {
    list(receipt, "unresolved")
        .iter()
        .filter(|issue| text(issue, "picture_profile") == Some(PROFILE))
}

pub(super) fn prepare<'a>(
    builder: &mut Builder<'a>,
    read: &'a ReadResult,
    sequence: usize,
    seq: &'a SourceSequence,
    receipt: &'a Value,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<(), PrepareIssue> {
    if !valid_overlay_sequence(seq, receipt) {
        return Ok(());
    }
    let bin = *read
        .sequence_bins
        .get(&seq.id)
        .ok_or("missing source bin assignment")?;
    let mut owners = HashSet::new();
    for issue in rows(receipt) {
        let owner = text(issue, "owner").ok_or("missing Matte Key occurrence")?;
        let length = number(issue, "record_length")
            .filter(|n| *n > 0)
            .ok_or("unresolved Matte Key length")?;
        let frame = issue
            .get("record_start")
            .and_then(|at| number(at, "value"))
            .filter(|n| *n >= 0)
            .ok_or("unresolved Matte Key position")?;
        let track = number(issue, "track")
            .filter(|n| (2..=64).contains(n))
            .ok_or("invalid Matte Key track")?;
        let (fill, alpha, import, invert) = still_sources(issue, length, OVERLAY_RATE)
            .filter(|(fill, alpha, _, _)| fill != alpha)
            .ok_or("contradictory split Matte Key inputs")?;
        if !owners.insert(owner)
            || text(issue, "effect_id") != Some(MATTE)
            || text(issue, "record_rate") != Some(OVERLAY_RATE)
            || number(issue, "kind") != Some(1)
        {
            return Err("contradictory split Matte Key occurrence or record clock".into());
        }
        let start = frame
            .checked_mul(OVERLAY_FRAME)
            .ok_or("Matte Key position exceeds Premiere timing bounds")?;
        let duration = length
            .checked_mul(OVERLAY_FRAME)
            .ok_or("Matte Key duration exceeds Premiere timing bounds")?;
        start
            .checked_add(duration)
            .ok_or("Matte Key end exceeds Premiere timing bounds")?;
        let omission = match builder.inspect(import, PngRole::BinaryMatte, check) {
            Ok(source) => match source.alpha_shapes[usize::from(invert)] {
                Some(shape) => {
                    let alpha_path = source.input.path.clone();
                    match prepare_fill(builder, fill, bin, length, duration, ffprobe, check)? {
                        Some(fill) => {
                            builder.split.used_alpha.insert(alpha_path);
                            builder.plan.picture.split_edits.push(PreparedSplitMatte {
                                sequence,
                                track,
                                start,
                                duration,
                                fill,
                                rectangle: shape.outer,
                                hole: shape.hole,
                            });
                            None
                        }
                        None => {
                            Some("The JPEG fill is unavailable or has no verified still binding")
                        }
                    }
                }
                None => Some(
                    source
                        .overlay_alpha_issue
                        .unwrap_or("The selected PNG alpha requires one rectangle with at most one enclosed rectangular hole"),
                ),
            },
            Err(PngVerificationError::MissingOriginal) => {
                Some("The original PNG alpha is unavailable")
            }
            Err(error) => {
                return Err(format!(
                    "V{track} at record frame {frame}: invalid PNG alpha for {alpha}: {error}"
                )
                .into())
            }
        };
        let unit = if length == 1 { "frame" } else { "frames" };
        let span =
            format!("V{track} at record frame {frame} for {length} {unit} ({OVERLAY_RATE} fps)");
        if let Some(reason) = omission {
            builder.plan.overlay_omissions.push(Loss {
                object: owner.into(),
                aspect: "picture_effect".into(),
                reason: format!(
                    "{span}: split Matte Key omitted. {reason}. The lower picture is retained."
                ),
            });
        }
        builder.plan.replace_loss(
            &seq.id,
            "picture_effect",
            format!(
                "{span}: {}",
                text(issue, "reason").ok_or("missing split Matte Key reason")?
            ),
        );
        builder.plan.recovered_reader_notes.insert(format!(
            "{:?}: V{track} at frame {frame}: Matte Key (EFF2_2DMATTE) not converted",
            seq.name
        ));
    }
    Ok(())
}

fn prepare_fill<'a>(
    builder: &mut Builder<'a>,
    mob: &str,
    sequence_bin: usize,
    length: i64,
    duration: i64,
    ffprobe: &Path,
    check: &mut dyn FnMut() -> Result<(), String>,
) -> Result<Option<usize>, String> {
    let Some(umid) = mob
        .strip_prefix("urn:smpte:umid:")
        .map(|id| id.replace('.', "").to_ascii_uppercase())
    else {
        return Err("split Matte Key fill has an invalid master identity".into());
    };
    let (key, path, name, existing_clip, managed_start) =
        if let Some((item, path)) = builder.split.linked_jpeg(&umid, duration) {
            let id = item.id.strip_prefix("clip:").unwrap_or(&item.id).to_owned();
            (
                (sequence_bin, umid),
                path.to_owned(),
                item.name.clone(),
                Some(id),
                None,
            )
        } else {
            let Some(ImportedFill { bin, name, import, managed }) = imported_fill(builder, mob, &umid) else {
                return Ok(None);
            };
            let Some(saved) = locators(import).filter(|paths| {
                paths.iter().all(|path| {
                    Path::new(path).extension().is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("jpg") || ext.eq_ignore_ascii_case("jpeg")
                    })
                })
            }) else {
                return Ok(None);
            };
            let hold = number(import, "master_length");
            if hold.is_none_or(|n| n < length)
                || number(import, "source_length") != hold
                || number(import, "track") != Some(1)
                || number(import, "file_track") != Some(1)
                || number(import, "file_in") != Some(0)
                || text(import, "rate") != Some(OVERLAY_RATE)
                || number(import, "import_aspect") != Some(2)
                || number(import, "color_level") != Some(1)
            {
                return Ok(None);
            }
            let name = name.to_owned();
            if !builder.split.imports.contains_key(&saved) {
                let source = verify_imported_jpeg(&saved, builder.mappings, ffprobe, check)
                    .map_err(|error| format!("JPEG fill {}: {error}", saved.join(" or ")))?;
                if let Some(source) = &source {
                    if builder
                        .physical
                        .get(&source.path)
                        .is_some_and(|other| *other != saved)
                    {
                        return Err("conflicting saved originals map to the JPEG fill".into());
                    }
                    builder.physical.insert(source.path.clone(), saved.clone());
                }
                builder.split.imports.insert(saved.clone(), source);
            }
            if let Some(source) = &builder.split.imports[&saved] {
                ((bin, umid), source.path.clone(), name, None, None)
            } else {
                let Some((item, media, native)) = managed else {
                    return Ok(None);
                };
                let Some((path, start, package, count)) = managed_binding((media, native)) else {
                    return Ok(None);
                };
                use std::collections::hash_map::Entry;
                match builder.split.managed_inputs.entry(path) {
                    Entry::Occupied(entry) => {
                        if (&entry.get().0, entry.get().1) != (&package, count) {
                            return Err("the same Matte Key MXF has conflicting package or clock claims".into());
                        }
                    }
                    Entry::Vacant(entry) => {
                        let verified = verify_imported_jpeg_mxf(path, ffprobe, package, count, check)
                            .map_err(|error| format!("JPEG fill {name:?} at {}: {error}", path.display()))?;
                        entry.insert((package, count, verified));
                    }
                }
                if !builder.split.ordinary_sources.contains(item.id.as_str()) {
                    builder.plan.managed_fill_only.insert(item.id.clone());
                }
                ((bin, umid), path.to_owned(), name, None, Some(start))
            }
        };
    if let Some(&index) = builder.split.fills.get(&key) {
        return Ok(Some(index));
    }
    let index = builder.plan.picture.jpeg_fills.len();
    builder.plan.picture.jpeg_fills.push(PreparedJpegFill {
        bin: key.0,
        path,
        name,
        existing_clip,
        managed_start,
    });
    builder.split.fills.insert(key, index);
    Ok(Some(index))
}

struct ImportedFill<'a> {
    bin: usize,
    name: &'a str,
    import: &'a Value,
    managed: Option<(&'a SourceItem, &'a SourceMedia, &'a Value)>,
}

fn imported_fill<'a>(
    builder: &Builder<'a>,
    mob: &str,
    umid: &str,
) -> Option<ImportedFill<'a>> {
    let &(bin, import) = builder.split.saved_imports.get(mob)?.as_ref()?;
    if matches!(builder.split.items.get(umid), Some(None)) {
        return None;
    }
    let package = text(import, "file_mob")?
        .strip_prefix("urn:smpte:umid:")?
        .replace('.', "")
        .to_ascii_uppercase();
    let (name, master, descriptor, managed) = if let Some(Some(item)) = builder.split.items.get(umid) {
        let master = *builder.split.native.get(item.id.as_str())?;
        if text(master, "occurrence")? != text(import, "id")? {
            return None;
        }
        let [binding] = item.media.as_slice() else {
            return None;
        };
        let media = builder.split.media.get(binding.media.as_str())?;
        let native = *builder.split.native.get(media.id.as_str())?;
        if media.kind != StreamKind::Video
            || media.original.is_some()
            || binding.source_offset != Some(Rational::integer(0))
            || text(native, "file_package") != Some(&package)
            || number(native, "native_track") != Some(1)
        {
            return None;
        }
        (item.name.as_str(), master, native.get("descriptor")?, Some((*item, *media, native)))
    } else {
        let still = builder.stills.get(mob)?;
        let [track] = list(still.master, "tracks") else {
            return None;
        };
        if text(track, "file_package") != Some(&package)
            || number(track, "kind") != Some(1)
            || number(track, "index") != Some(1)
        {
            return None;
        }
        (
            text(still.master, "name")?,
            still.master,
            track.get("descriptor")?,
            None,
        )
    };
    (master.get("input_color_transforms").is_none()
        && number(descriptor, "width") == Some(1920)
        && number(descriptor, "height") == Some(1080)
        && list(descriptor, "display_aspect") == [Value::Int(16), Value::Int(9)])
    .then_some(ImportedFill { bin, name, import, managed })
}

fn managed_binding<'a>((media, native): (&'a SourceMedia, &'a Value)) -> Option<(&'a Path, i64, &'a str, u64)> {
    let [path] = media.locations.as_slice() else { return None };
    let rate = Rational::new(24_000, 1001)?;
    let clock = media.clock.as_ref()?;
    let descriptor = native.get("descriptor")?;
    let start = media.start?.to_units(TICKS_PER_SECOND)?;
    (media.online == Some(true)
        && media.codec.as_deref() == Some("AVdn")
        && media.rate == Some(rate)
        && clock.rate == rate && clock.display_base == 24 && !clock.drop
        && start >= 0 && start % OVERLAY_FRAME == 0
        && start / OVERLAY_FRAME == i64::try_from(clock.effective_count).ok()?
        && text(native, "descriptor_rate") == Some(OVERLAY_RATE)
        && number(native, "descriptor_length") == Some(1)
        && text(descriptor, "compression") == Some("AVHD")
        && number(descriptor, "resolution_id") == Some(1237))
    .then_some((Path::new(path), start, text(native, "file_package")?, clock.effective_count))
}

pub(super) fn position_graphics(plan: &mut Plan, texts: &mut [PreparedText]) {
    // A graphic on the next lane with identical bounds can sit inside a
    // full-width lower strip. This is a destination layout, independent of label text.
    let strips: HashMap<_, _> =
        plan.picture
            .split_edits
            .iter()
            .filter_map(|edit| {
                let [left, top, right, bottom] = edit.rectangle;
                (left == 0 && right == 1920 && bottom == 1080 && (756..=960).contains(&top))
                    .then_some((
                        (edit.sequence, edit.track + 1, edit.start, edit.duration),
                        top,
                    ))
            })
            .collect();
    for text in texts.iter_mut().filter(|text| {
        text.kind == PreparedTextKind::Graphic
            && !text.content.contains('\n')
            && text.content.chars().count() <= 40
    }) {
        if let Some(top) = strips.get(&(text.sequence, text.track, text.start, text.duration)) {
            text.graphic_position = Some((0.10, (f64::from(*top + 1080) / 2.0 + 30.0) / 1080.0));
            plan.replace_loss(&text.source_id, "text_destination_style",
                "Text and timing transfer as editable point text with Premiere's ArialMT/48 pt styling at normalized position (0.1, 0.35).".into());
            plan.overlay_omissions.push(Loss {
                object: text.source_id.clone(), aspect: "text_destination_style".into(),
                reason: "Premiere uses default text styling and positions the title on the matching lower matte strip.".into(),
            });
        }
    }
}
