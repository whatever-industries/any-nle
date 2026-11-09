use std::{env, path::PathBuf};

fn main() {
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
