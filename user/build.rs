//! Links every program with user.ld (an absolute path, as the linker
//! does not run from this directory).

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    println!("cargo:rustc-link-arg-bins=-T{dir}/user.ld");
    println!("cargo:rerun-if-changed=user.ld");
}
