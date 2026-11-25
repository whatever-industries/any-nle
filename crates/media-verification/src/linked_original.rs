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
            let matching_hold = media.rate.is_some_and(|rate| {
                let supported = rate == Rational::integer(24)
                    || original.container == "jpeg" && (rate.num(), rate.den()) == (24_000, 1001);
                supported
                    && Rational::new(rate.den().into(), rate.num().into()) == Some(original.time_base)
                    && Rational::new(
                        i128::from(media.duration.num()) * i128::from(rate.num()),
                        i128::from(media.duration.den()) * i128::from(rate.den()),
                    ).is_some_and(|frames| frames.num() > 0 && frames.den() == 1)
            });
            if !raster
                || media.online != Some(true)
                || original.stream_index != 0
                || !matching_hold
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

#[cfg(test)]
mod tests {
    use super::*;
    use graph::source::OriginalStream;

    #[test]
    fn fractional_jpeg_hold_requires_its_exact_frame_grid() {
        let mut jpeg = SourceMedia {
            id: "jpeg".into(),
            kind: StreamKind::Video,
            locations: vec!["/Synthetic/hold.jpg".into()],
            codec: Some("jpeg".into()),
            frame_size: Some((1920, 1080)),
            rate: Rational::new(24_000, 1001),
            sample_rate: None,
            bits: None,
            duration: Rational::new(720 * 1001, 24_000).unwrap(),
            start: Some(Rational::ZERO),
            stream: Some(1),
            online: Some(true),
            clock: None,
            original: Some(OriginalStream {
                saved_locators: vec!["/Synthetic/hold.jpg".into()],
                container: "jpeg".into(),
                stream_index: 0,
                time_base: Rational::new(1001, 24_000).unwrap(),
                start_pts: 0,
                duration_ts: 1,
                channels: None,
                channel_index: None,
                sample_representation: None,
            }),
        };
        assert!(qualify(std::slice::from_ref(&jpeg)).is_ok());
        jpeg.duration = Rational::integer(30);
        assert!(qualify(std::slice::from_ref(&jpeg)).is_err());
        jpeg.duration = Rational::new(720 * 1001, 24_000).unwrap();
        jpeg.original.as_mut().unwrap().time_base = Rational::new(1, 24).unwrap();
        assert!(qualify(&[jpeg]).is_err());
    }
}
