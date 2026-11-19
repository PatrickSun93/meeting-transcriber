# Vendored crates

## eframe 0.37.0 (`eframe/`)

The published eframe 0.37.0 (MIT OR Apache-2.0, by Emil Ernerfeldt and the
egui contributors, <https://github.com/emilk/egui>), used through
`[patch.crates-io]` in `Cargo.toml`, with one change from egui's main
branch: [#8832](https://github.com/emilk/egui/pull/8832) (commit
`9bb68fc5d2`), "Fix panic when using immediate viewports", in
`src/native/run.rs` (`new_events`).

Without it, eframe runs a hidden or minimized window's frame from
`new_events` without making the event loop current, so a window the app
opens in that frame (`show_viewport_immediate`: the screenshot overlay, the
countdown, pins, the scrolling and recording windows) cannot be created,
and egui panics with "egui backend is implemented incorrectly - the user
callback was never called". On Windows this crashed the app on every
screenshot taken with its window hidden.

Remove this folder and the patch once an eframe release includes #8832.
