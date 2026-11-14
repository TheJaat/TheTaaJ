//! Tells the linker to use linker.ld.
//!
//! A build script rather than a flag in .cargo/config.toml because the
//! path has to be absolute: the linker does not run from this directory.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg-bins=-T{dir}/linker.ld");
    println!("cargo:rerun-if-changed=linker.ld");
}
