fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let script = std::path::Path::new(&dir).join("../link.ld");
    println!("cargo:rustc-link-arg=-T{}", script.display());
    println!("cargo:rustc-link-arg=-no-pie");
    println!("cargo:rustc-link-arg=-zmax-page-size=0x1000");
    println!("cargo:rerun-if-changed={}", script.display());
}
