// screencapturekit and apple-metal link Swift static libraries, which load the
// Swift runtime (e.g. libswift_Concurrency.dylib) through @rpath. Their build
// scripts add the rpath, but Cargo does not pass a dependency's link args on to
// the final binary, so the app crashed at launch with "no LC_RPATH's found".
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // The Swift runtime ships with macOS (12+) here.
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
        // Room for Swift libraries bundled inside the .app if ever needed.
        println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    }
}
