//! Bounded encodings used by the qualified native skeleton.
use crate::Result;

pub(crate) enum Qt<'a> {
    Int(i32),
    Uint(u32),
    Long(i64),
    Double(f64),
    Text(&'a str),
    Bytes(&'a [u8]),
}

fn sized(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

pub(crate) fn qt(fields: &[(&str, Qt<'_>)]) -> Vec<u8> {
    let mut out = 1u32.to_be_bytes().to_vec();
    out.extend_from_slice(&(fields.len() as u32).to_be_bytes());
    for (name, field) in fields {
        sized(&mut out, &utf16(name));
        let ty: u32 = match field {
            Qt::Int(_) => 2,
            Qt::Uint(_) => 3,
            Qt::Long(_) => 4,
            Qt::Double(_) => 6,
            Qt::Text(_) => 10,
            Qt::Bytes(_) => 12,
        };
        out.extend_from_slice(&ty.to_be_bytes());
        out.push(0);
        match field {
            Qt::Int(n) => out.extend_from_slice(&n.to_be_bytes()),
            Qt::Uint(n) => out.extend_from_slice(&n.to_be_bytes()),
            Qt::Long(n) => out.extend_from_slice(&n.to_be_bytes()),
            Qt::Double(n) => out.extend_from_slice(&n.to_be_bytes()),
            Qt::Text(text) => sized(&mut out, &utf16(text)),
            Qt::Bytes(bytes) => sized(&mut out, bytes),
        }
    }
    out
}

pub(crate) fn take_varint(data: &[u8], at: &mut usize) -> Result<u64> {
    let mut out = 0;
    for shift in (0..=63).step_by(7) {
        let byte = *data.get(*at).ok_or("truncated protobuf varint")?;
        *at += 1;
        if shift == 63 && byte > 1 {
            return Err("protobuf varint overflow".into());
        }
        out |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(out);
        }
    }
    Err("invalid protobuf varint".into())
}

fn varint(mut n: u64, out: &mut Vec<u8>) {
    while n >= 128 {
        out.push((n as u8) | 128);
        n >>= 7;
    }
    out.push(n as u8);
}

pub(crate) fn pb_int(field: u64, value: u64, out: &mut Vec<u8>) {
    varint(field << 3, out);
    varint(value, out);
}

pub(crate) fn pb_bytes(field: u64, value: &[u8], out: &mut Vec<u8>) {
    varint(field << 3 | 2, out);
    varint(value.len() as u64, out);
    out.extend_from_slice(value);
}

/// Change exactly one existing length-delimited field while preserving other bytes.
pub(crate) fn replace_bytes(
    data: &[u8],
    field: u64,
    change: impl FnOnce(&[u8]) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    rewrite_bytes(data, field, |old| {
        Ok(Some(change(old.ok_or("missing protobuf field")?)?))
    })
}

pub(crate) fn rewrite_bytes(
    data: &[u8],
    field: u64,
    change: impl FnOnce(Option<&[u8]>) -> Result<Option<Vec<u8>>>,
) -> Result<Vec<u8>> {
    let mut at = 0;
    let mut found = None;
    while at < data.len() {
        let begin = at;
        let key = take_varint(data, &mut at)?;
        let wire = key & 7;
        let count = match wire {
            0 => {
                take_varint(data, &mut at)?;
                0
            }
            1 => 8,
            2 => usize::try_from(take_varint(data, &mut at)?)?,
            5 => 4,
            _ => return Err("unsupported protobuf wire type".into()),
        };
        let end = at
            .checked_add(count)
            .filter(|end| *end <= data.len())
            .ok_or("bad protobuf span")?;
        if key >> 3 == field && (wire != 2 || found.replace((begin, at, end)).is_some()) {
            return Err("ambiguous protobuf field".into());
        }
        at = end;
    }
    let (begin, start, end) = found.unwrap_or((data.len(), data.len(), data.len()));
    let mut out = data[..begin].to_vec();
    if let Some(updated) = change(found.map(|_| &data[start..end]))? {
        pb_bytes(field, &updated, &mut out);
    }
    out.extend_from_slice(&data[end..]);
    Ok(out)
}

pub(crate) fn unpack(encoded: &str) -> Result<Vec<u8>> {
    let b = hex::decode(encoded)?;
    if b.len() < 9
        || b[..4] != 2u32.to_be_bytes()
        || u32::from_be_bytes(b[4..8].try_into()?) as usize != b.len() - 8
    {
        return Err("malformed Resolve blob header".into());
    }
    match b[8] {
        0x80 if b.len() <= 65536 => Ok(b[9..].to_vec()),
        0x81 => Ok(zstd::bulk::decompress(&b[9..], 65536)?),
        _ => Err("unsupported Resolve blob encoding".into()),
    }
}

pub(crate) fn pack(body: &[u8]) -> Result<String> {
    let payload = zstd::bulk::compress(body, 3)?;
    let mut out = 2u32.to_be_bytes().to_vec();
    out.extend_from_slice(&u32::try_from(payload.len() + 1)?.to_be_bytes());
    out.push(0x81);
    out.extend_from_slice(&payload);
    Ok(hex::encode(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_blob_patch_preserves_siblings() {
        let old = [0x0a, 1, 55, 0x10, 1];
        let patch = replace_bytes(&old, 1, |_| Ok(vec![71, 72])).unwrap();
        assert_eq!(patch, [0x0a, 2, 71, 72, 0x10, 1]);
        assert_eq!(unpack(&pack(&patch).unwrap()).unwrap(), patch);
        assert!(replace_bytes(&[0x0a, 9], 1, |_| Ok(vec![])).is_err());
        assert!(replace_bytes(&[0x0a, 0, 0x0a, 0], 1, |_| Ok(vec![])).is_err());
        assert!(unpack("00000002000000018000").is_err());
    }
}
