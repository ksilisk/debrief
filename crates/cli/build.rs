fn main() {
    // screencapturekit's Swift bridge loads the Swift runtime through @rpath, and a
    // dependency's build script cannot add an rpath to our binary.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
}
