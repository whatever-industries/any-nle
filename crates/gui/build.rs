use resvg::{tiny_skia, usvg};
use std::{env, fs, path::PathBuf};

fn main() {
    const SIZE: u32 = 64;
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    for icon in ["arrow-right", "project", "media", "review", "layers", "activity"] {
        let path = format!("resources/{icon}.svg");
        println!("cargo:rerun-if-changed={path}");
        let svg = fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let tree = usvg::Tree::from_data(&svg, &usvg::Options::default())
            .unwrap_or_else(|e| panic!("parse {path}: {e}"));
        let scale = SIZE as f32 / tree.size().width();
        let mut pixels = tiny_skia::Pixmap::new(SIZE, SIZE).expect("allocate the icon image");
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixels.as_mut(),
        );
        fs::write(output.join(format!("{icon}.rgba")), pixels.data()).expect("embed icon image");
    }

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let icon = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("crate directory"))
        .join("../../packaging/icons/any-nle.res");
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rustc-link-arg-bin=any-nle-gui={}", icon.display());
}
