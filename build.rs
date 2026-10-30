// The ScreenCaptureKit bridge is written in Swift and loads the Swift runtime
// (libswift_Concurrency.dylib and friends) through @rpath. macOS ships that
// runtime in /usr/lib/swift, but nothing adds it as an rpath, so without this
// the app dies in dyld at launch: "Library not loaded: @rpath/libswift_...".
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
