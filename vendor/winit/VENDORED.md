# Vendored winit 0.30.13

The crates.io release of winit 0.30.13, unchanged except that tvOS builds as iOS, so `rune`
compiles for `aarch64-apple-tvos` and `aarch64-apple-tvos-sim`. Upstream 0.30.13 files tvOS under
"free Unix" and pulls in calloop, rustix and Wayland, which don't build there.

- `build.rs`: `ios_platform` and `apple` include `target_os = "tvos"`.
- `Cargo.toml`: every `target_os = "ios"` dependency table includes tvOS; the examples, tests and
  benches entries are removed, as their files are.

Rune apps on tvOS host Rune through `rune::embed::LayerHost`, as on iOS, so winit's UIKit
backend compiles there but no Rune window runs on it. Drop this copy for the crates.io release
once a winit release Rune can move to builds for tvOS.
