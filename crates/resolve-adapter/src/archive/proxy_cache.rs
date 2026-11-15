//! Resolve 21.2 Media Pool attachment cache. Media is still bound to the original.
use super::*;
use avid_adapter::proxy::Attachment;

fn cache(proxy: &Attachment, video: bool) -> Result<Vec<u8>> {
    let (folder, file) = media_path_parts(&proxy.path)?;
    let mut data = Vec::new();
    pb_bytes(1, folder.as_bytes(), &mut data);
    pb_bytes(2, file.as_bytes(), &mut data);
    pb_bytes(
        5,
        if video {
            b"dnxhd_1080p36"
        } else {
            b"Linear PCM"
        },
        &mut data,
    );
    if video {
        pb_bytes(6, file.as_bytes(), &mut data);
        pb_bytes(7, uid().as_bytes(), &mut data);
    }
    pb_int(15, 8, &mut data);
    pb_int(16, 100, &mut data);
    pb_int(
        18,
        if proxy.bits.is_none() { 16388 } else { 16384 },
        &mut data,
    );
    Ok(hex::decode(pack(&data)?)?)
}

pub(super) fn build(proxy: &Attachment) -> Result<Vec<u8>> {
    let fps = proxy.rate.num() as f64 / proxy.rate.den() as f64;
    let mut rate = [0; 16];
    rate[..8].copy_from_slice(&fps.to_le_bytes());
    let timecode = crate::linked::non_drop_24(proxy.start);
    let timing = qt(&[
        ("UniqueId", Text(&uid())),
        ("Timecode", Text(&timecode)),
        ("StartFrame", Int(0)),
        ("NumFrames", Int(i32::try_from(proxy.frames)?)),
        ("FrameRate", Bytes(&rate)),
        ("DbType", Text("BtVideoTime")),
    ]);
    let mut raster = [0; 16];
    raster[..8].copy_from_slice(&1920u64.to_be_bytes());
    raster[8..].copy_from_slice(&1080u64.to_be_bytes());
    let geometry = qt(&[
        ("UniqueId", Text(&uid())),
        ("Resolution", Bytes(&raster)),
        ("FrameSize", Int(4163584)),
        ("DbType", Text("BtGeometry")),
    ]);
    let metadata = qt(&[
        ("UniqueId", Text(&uid())),
        ("DbType", Text("BtVideoMetadata")),
    ]);
    let state = qt(&[
        ("UniqueId", Text(&uid())),
        ("DbType", Text("BtVideoProxy")),
        ("DataManagerID", Int(50)),
    ]);
    let video = qt(&[
        ("UniqueId", Text(&uid())),
        ("Time", Bytes(&timing)),
        ("Geometry", Bytes(&geometry)),
        ("Clip", Bytes(&cache(proxy, true)?)),
        ("Proxy", Bytes(&state)),
        (
            "Radiometry",
            Bytes(&hex::decode("80087810022810880132980100d80100")?),
        ),
        ("VideoMetadata", Bytes(&metadata)),
        ("DbType", Text("BtVideoInfo")),
    ]);
    if let Some(bits) = proxy.bits {
        let count = proxy
            .frames
            .checked_mul(2002)
            .ok_or("proxy sample extent overflows")?;
        let start = proxy.start as f64 / fps;
        let span = extent(start, count as f64 / 48000.0);
        let track = qt(&[
            ("UniqueId", Text(&uid())),
            ("StartTime", Double(start)),
            ("SampleRate", Uint(48000)),
            ("NumChannels", Int(2)),
            ("IdxTrack", Int(0)),
            ("Duration", Long(count)),
            ("DbType", Text("BtAudioTrack")),
            ("CodecName", Text("Linear PCM")),
            ("ChannelLayout", Int(2)),
            ("BitDepth", Uint(if bits == 16 { 1 } else { 2 })),
        ]);
        let tracks = qt(&[("0", Bytes(&track))]);
        let audio = qt(&[
            ("UniqueId", Text(&uid())),
            ("TracksBA", Bytes(&tracks)),
            ("DbType", Text("BtAudioInfo")),
            ("Clip", Bytes(&cache(proxy, false)?)),
        ]);
        Ok(qt(&[
            ("VIDEO_INFO", Bytes(&video)),
            ("AUDIO_TRACKS_EXTENTS", ByteList(&[&span])),
            ("AUDIO_INFO_VEC", ByteList(&[&audio])),
        ]))
    } else {
        Ok(qt(&[("VIDEO_INFO", Bytes(&video))]))
    }
}
