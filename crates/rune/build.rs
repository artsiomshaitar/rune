//! tvOS only: winit's `objc2-foundation` 0.2 pulls in the `dispatch` crate, which links
//! `-ldispatch` on every Apple target but macOS and iOS. tvOS has no such library to link;
//! libdispatch is part of libSystem there. An empty `libdispatch.a` on the link path satisfies
//! the flag without adding anything, and link paths from here reach every binary using Rune.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var_os("CARGO_CFG_TARGET_OS").is_none_or(|os| os != "tvos") {
        return;
    }
    let Some(out) = std::env::var_os("OUT_DIR") else { return };
    let dir = std::path::Path::new(&out).join("tvos-link");
    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(dir.join("libdispatch.a"), b"!<arch>\n").is_ok() {
        println!("cargo::rustc-link-search=native={}", dir.display());
    }
}
