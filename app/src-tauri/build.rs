fn main() {
    // Same as crates/cli/build.rs: screencapturekit's Swift bridge finds the Swift
    // runtime through @rpath, and only the binary's own build script can add one.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
