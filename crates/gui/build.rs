use resvg::{tiny_skia, usvg};
use std::{env, fs, path::PathBuf};

fn main() {
    const SVG: &str = "resources/arrow-right.svg";
    const SIZE: u32 = 64;
    println!("cargo:rerun-if-changed={SVG}");
    let svg = fs::read(SVG).expect("read the arrow SVG");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("parse the arrow SVG");
    let scale = SIZE as f32 / tree.size().width();
    let mut pixels = tiny_skia::Pixmap::new(SIZE, SIZE).expect("allocate the arrow image");
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixels.as_mut(),
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"));
    fs::write(output.join("arrow-right.rgba"), pixels.data()).expect("embed the arrow image");

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
