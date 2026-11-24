//! Shared admission limits for linked-original conversion. Adapters still
//! verify the actual file and qualify their own clocks, codecs and routing.

use graph::{source::SourceMedia, GraphError, Rational, StreamKind};

/// Apply after graph validation and before using or replacing saved stream facts.
pub fn qualify(media: &[SourceMedia]) -> graph::R<()> {
    for media in media {
        let Some(original) = &media.original else {
            continue;
        };
        let refuse = |reason: &str| GraphError(format!("source media {:?}: {reason}", media.id));
        if original.start_pts != 0 {
            return Err(refuse("unsupported original stream origin or extent"));
        }
        if matches!(original.container.as_str(), "jpeg" | "png" | "tiff") {
            let raster = match original.container.as_str() {
                "tiff" => media.frame_size == Some((1920, 64)),
                _ => media.frame_size.is_some_and(|(w, h)| {
                    (16..=8192).contains(&w) && (16..=8192).contains(&h) && w * h <= 16_777_216
                }),
            };
            if !raster
                || media.online != Some(true)
                || original.stream_index != 0
                || original.time_base != Rational::new(1, 24).unwrap()
                || media.rate != Some(Rational::integer(24))
                || !media.duration.to_units(24).is_some_and(|frames| frames > 0)
            {
                return Err(refuse("invalid original still or editorial hold"));
            }
        }
        if media.kind != StreamKind::Audio {
            continue;
        }
        let representation = original.sample_representation.as_deref();
        let eight_mic_wav = original.channels == Some(8)
            && original.container == "wav"
            && media.sample_rate == Some(48_000)
            && representation == Some("pcm16");
        if !matches!(original.channels, Some(1 | 2)) && !eight_mic_wav {
            return Err(refuse("invalid physical audio channel"));
        }
        let supported = match representation {
            Some("aac" | "pcm16" | "pcm24") => true,
            Some("mp3") => {
                original.container == "mp3"
                    && media.codec.as_deref() == Some("mp3")
                    && media.online == Some(true)
                    && original.stream_index == 0
                    && media.sample_rate.is_some_and(|rate| {
                        matches!(rate, 44_100 | 48_000)
                            && Rational::new(1, rate.into()) == Some(original.time_base)
                    })
            }
            Some("pcm16be") => {
                original.container == "mov"
                    && original.stream_index == 1
                    && original.channels == Some(2)
                    && media.sample_rate == Some(48_000)
                    && original.time_base == Rational::new(1, 48_000).unwrap()
            }
            _ => false,
        };
        if !supported {
            return Err(refuse(
                "audio sample representation contradicts bit depth or media profile",
            ));
        }
    }
    Ok(())
}
