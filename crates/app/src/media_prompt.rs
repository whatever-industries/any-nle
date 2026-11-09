//! Native media preparation UI. Dialogs run synchronously on the CLI's main thread.
use std::path::{Path, PathBuf};

/// Pick an unused child name without creating it. Plan::prepare and execute retain
/// authority over source-root exclusions, collisions and exclusive directory creation.
fn new_directory(parent: &Path) -> Result<PathBuf, String> {
    for n in 1..=10_000 {
        let name = if n == 1 {
            "OP1a Media".into()
        } else {
            format!("OP1a Media {n}")
        };
        let candidate = parent.join(name);
        match std::fs::symlink_metadata(&candidate) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => continue,
            Err(e) => return Err(format!("media destination: {e}")),
        }
    }
    Err("too many existing OP1a Media folders; choose another location".into())
}

pub fn size_label(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} bytes")
    } else {
        format!("{size:.2} {}", UNITS[unit])
    }
}

/// A CLI has no AppKit launch cycle. Finish it once so modal dialogs service
/// normal application/accessibility events as well as their own controls.
#[cfg(target_os = "macos")]
fn native_ui() -> Result<objc2::MainThreadMarker, String> {
    let mtm = objc2::MainThreadMarker::new().ok_or("media dialogs require the main thread")?;
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        let app = NSApplication::sharedApplication(mtm);
        if !app.isRunning() {
            app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
            app.finishLaunching();
        }
    });
    Ok(mtm)
}

#[cfg(target_os = "macos")]
pub fn choose_directory(project: &Path) -> Result<Option<PathBuf>, String> {
    native_ui()?;
    let initial = project
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let selection = rfd::FileDialog::new()
        .set_title("Creates new media. Choose where to save a new OP1a Media folder")
        .set_directory(initial)
        .set_can_create_directories(false)
        .pick_folder();
    selection.map(|parent| new_directory(&parent)).transpose()
}

#[cfg(target_os = "macos")]
pub fn confirm(review: &str) -> Result<bool, String> {
    use objc2_app_kit::{NSAlert, NSAlertFirstButtonReturn};
    use objc2_foundation::NSString;
    let alert = NSAlert::new(native_ui()?);
    alert.setMessageText(&NSString::from_str("Any NLE — Review new media"));
    alert.setInformativeText(&NSString::from_str(review));
    alert.addButtonWithTitle(&NSString::from_str("Create media"));
    alert.addButtonWithTitle(&NSString::from_str("Cancel"));
    Ok(alert.runModal() == NSAlertFirstButtonReturn)
}

#[cfg(not(target_os = "macos"))]
pub fn choose_directory(_: &Path) -> Result<Option<PathBuf>, String> {
    Err("native media folder prompting currently requires macOS; use --rewrap-op1a NEW_DIR".into())
}

#[cfg(not(target_os = "macos"))]
pub fn confirm(_: &str) -> Result<bool, String> {
    Err("native media review currently requires macOS".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn naming_preserves_existing_files_folders_and_dangling_links() {
        let parent = std::env::temp_dir().join(format!("anynle-picker-{}", std::process::id()));
        std::fs::create_dir(&parent).unwrap();
        std::fs::write(parent.join("OP1a Media"), b"preserve").unwrap();
        std::fs::create_dir(parent.join("OP1a Media 2")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(parent.join("absent"), parent.join("OP1a Media 3")).unwrap();
        let chosen = new_directory(&parent).unwrap();
        #[cfg(unix)]
        assert_eq!(chosen, parent.join("OP1a Media 4"));
        assert!(!chosen.exists());
        assert_eq!(
            std::fs::read(parent.join("OP1a Media")).unwrap(),
            b"preserve"
        );
        std::fs::remove_dir_all(&parent).unwrap();
    }
    #[test]
    fn sizes_include_zero_and_large_volumes() {
        assert_eq!(size_label(0), "0 bytes");
        assert_eq!(size_label(1024), "1.00 KiB");
        assert_eq!(size_label(3 << 30), "3.00 GiB");
        assert_eq!(size_label(u64::MAX), "16777216.00 TiB");
    }
}
