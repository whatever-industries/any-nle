use std::{fs::File, io::Read, path::Path};

pub fn read(path: &Path, max: usize) -> Result<String, Box<dyn std::error::Error>> {
    if !std::fs::metadata(path)?.is_file() {
        return Err("qualification input must be a regular file".into());
    }
    let mut text = String::new();
    File::open(path)?
        .take(max as u64 + 1)
        .read_to_string(&mut text)?;
    if text.len() > max {
        return Err("qualification input exceeds its byte limit".into());
    }
    Ok(text)
}
