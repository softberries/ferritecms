fn main() {
    // 40 MiB = 640 wasm pages.
    println!("cargo:rustc-link-arg-cdylib=--initial-memory=41943040");
}
