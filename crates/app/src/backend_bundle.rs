//! Verified adjacent media tools. Explicit overrides are separate from packaged lookup.
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackendTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub bundled: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    ffmpeg_version: String,
    target: String,
    license: String,
    source: Source,
    configure: Vec<String>,
    tools: Tools,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    url: String,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tools {
    ffmpeg: Tool,
    ffprobe: Tool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tool {
    file: String,
    sha256: String,
}
fn target() -> Result<String, String> {
    let arch = std::env::consts::ARCH;
    if !matches!(arch, "aarch64" | "x86_64") {
        return Err("no bundled media tools for this architecture".into());
    }
    let suffix = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        "linux" => "unknown-linux-gnu",
        _ => return Err("no bundled media tools for this OS".into()),
    };
    Ok(format!("{arch}-{suffix}"))
}
fn hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn tool(dir: &Path, t: &Tool, name: &str) -> Result<PathBuf, String> {
    let expected = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    if t.file != expected || !hex(&t.sha256) {
        return Err(format!("invalid bundled {name} manifest entry"));
    }
    let p = dir.join(&t.file);
    let meta = std::fs::symlink_metadata(&p).map_err(|e| format!("bundled {name}: {e}"))?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(format!("bundled {name} must be a regular adjacent file"));
    }
    let f = std::fs::File::open(&p).map_err(|e| e.to_string())?;
    let actual =
        prproj::hash::sha256_reader(std::io::BufReader::new(f)).map_err(|e| e.to_string())?;
    if !prproj::hash::hex(&actual).eq_ignore_ascii_case(&t.sha256) {
        return Err(format!("bundled {name} checksum mismatch"));
    }
    Ok(p)
}
pub fn verify_directory(dir: &Path) -> Result<BackendTools, String> {
    let p = dir.join("backend-manifest.json");
    let meta = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 65536 {
        return Err("invalid bundled backend manifest file".into());
    }
    let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
    let m: Manifest =
        serde_json::from_slice(&bytes).map_err(|e| format!("invalid backend manifest: {e}"))?;
    if m.schema != 1
        || m.ffmpeg_version != "8.1"
        || m.target != target()?
        || m.license != "LGPL-2.1-or-later"
        || m.source.sha256 != "b072aed6871998cce9b36e7774033105ca29e33632be5b6347f3206898e0756a"
        || m.source.url != "https://ffmpeg.org/releases/ffmpeg-8.1.tar.xz"
    {
        return Err("unsupported backend identity/license/source manifest".into());
    }
    if [
        "--disable-autodetect",
        "--disable-gpl",
        "--disable-nonfree",
        "--disable-version3",
        "--disable-network",
    ]
    .iter()
    .any(|flag| !m.configure.iter().any(|s| s == flag))
        || m.configure.len() > 256
        || m.configure.iter().any(|s| {
            s.len() > 2048
                || matches!(
                    s.split('=').next().unwrap_or(""),
                    "--enable-gpl" | "--enable-nonfree" | "--enable-version3"
                )
        })
    {
        return Err("bundled backend has unsupported build configuration".into());
    }
    Ok(BackendTools {
        ffmpeg: tool(dir, &m.tools.ffmpeg, "ffmpeg")?,
        ffprobe: tool(dir, &m.tools.ffprobe, "ffprobe")?,
        bundled: true,
    })
}
/// GUI calls this with require_bundle=true; unbundled CLI retains PATH compatibility.
pub fn resolve(
    ffmpeg: Option<&Path>,
    ffprobe: Option<&Path>,
    require_bundle: bool,
) -> Result<BackendTools, String> {
    if let (Some(ffmpeg), Some(ffprobe)) = (ffmpeg, ffprobe) {
        return Ok(BackendTools {
            ffmpeg: ffmpeg.into(),
            ffprobe: ffprobe.into(),
            bundled: false,
        });
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("cannot locate application directory")?;
    let bundled = match std::fs::symlink_metadata(dir.join("backend-manifest.json")) {
        Ok(_) => Some(verify_directory(dir)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("Cannot inspect bundled backend manifest: {e}")),
    };
    resolve_parts(ffmpeg, ffprobe, bundled, require_bundle)
}
fn resolve_parts(
    ffmpeg: Option<&Path>,
    ffprobe: Option<&Path>,
    bundle: Option<BackendTools>,
    require_bundle: bool,
) -> Result<BackendTools, String> {
    if require_bundle && bundle.is_none() && (ffmpeg.is_none() || ffprobe.is_none()) {
        return Err("Bundled media tools are missing. Reinstall a complete Any NLE package, or explicitly select both tools.".into());
    }
    let bundled = ffmpeg.is_none() && ffprobe.is_none() && bundle.is_some();
    Ok(BackendTools {
        ffmpeg: ffmpeg
            .map(Path::to_owned)
            .or_else(|| bundle.as_ref().map(|b| b.ffmpeg.clone()))
            .unwrap_or_else(|| "ffmpeg".into()),
        ffprobe: ffprobe
            .map(Path::to_owned)
            .or_else(|| bundle.as_ref().map(|b| b.ffprobe.clone()))
            .unwrap_or_else(|| "ffprobe".into()),
        bundled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_tool_override_preserves_verified_partner_and_refuses_missing_partner() {
        let b = BackendTools {
            ffmpeg: "/bundle/ffmpeg".into(),
            ffprobe: "/bundle/ffprobe".into(),
            bundled: true,
        };
        let r = resolve_parts(Some(Path::new("/override/ffmpeg")), None, Some(b), true).unwrap();
        assert_eq!(r.ffprobe, PathBuf::from("/bundle/ffprobe"));
        assert!(!r.bundled);
        assert!(resolve_parts(Some(Path::new("/override/ffmpeg")), None, None, true).is_err());
        assert_eq!(
            resolve_parts(None, None, None, false).unwrap().ffprobe,
            PathBuf::from("ffprobe")
        );
    }

    #[test]
    fn bundle_identity_hash_config_and_paths_are_enforced() {
        let dir = std::env::temp_dir().join(format!(
            "anynle-backend-bundle-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        let data = b"synthetic tool";
        for name in ["ffmpeg", "ffprobe"] {
            std::fs::write(
                dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)),
                data,
            )
            .unwrap();
        }
        let hash = prproj::hash::hex(&prproj::hash::sha256(data));
        let m = serde_json::json!({"schema":1,"ffmpeg_version":"8.1","target":target().unwrap(),"license":"LGPL-2.1-or-later","source":{"url":"https://ffmpeg.org/releases/ffmpeg-8.1.tar.xz","sha256":"b072aed6871998cce9b36e7774033105ca29e33632be5b6347f3206898e0756a"},"configure":["--disable-autodetect","--disable-gpl","--disable-nonfree","--disable-version3","--disable-network"],"tools":{"ffmpeg":{"file":format!("ffmpeg{}",std::env::consts::EXE_SUFFIX),"sha256":hash},"ffprobe":{"file":format!("ffprobe{}",std::env::consts::EXE_SUFFIX),"sha256":hash}}});
        let write = |v: &serde_json::Value| {
            std::fs::write(
                dir.join("backend-manifest.json"),
                serde_json::to_vec(v).unwrap(),
            )
            .unwrap()
        };
        write(&m);
        assert!(verify_directory(&dir).unwrap().bundled);
        for field in ["target", "ffmpeg_version", "license"] {
            let mut bad = m.clone();
            bad[field] = "wrong".into();
            write(&bad);
            assert!(verify_directory(&dir).is_err());
        }
        let mut bad = m.clone();
        bad["configure"] = serde_json::json!(["--enable-gpl"]);
        write(&bad);
        assert!(verify_directory(&dir).is_err());
        let mut bad = m.clone();
        bad["tools"]["ffmpeg"]["file"] = "../ffmpeg".into();
        write(&bad);
        assert!(verify_directory(&dir).is_err());
        for bad in [
            serde_json::json!({"unexpected":true}),
            serde_json::json!({"configure":[]}),
            serde_json::json!({"source":{"url":"https://example.invalid/source.tar.xz","sha256":"0".repeat(64)}}),
        ] {
            let mut v = m.clone();
            for (k, value) in bad.as_object().unwrap() {
                v[k] = value.clone();
            }
            write(&v);
            assert!(verify_directory(&dir).is_err());
        }
        write(&m);
        std::fs::write(
            dir.join(format!("ffprobe{}", std::env::consts::EXE_SUFFIX)),
            b"changed",
        )
        .unwrap();
        assert!(verify_directory(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
