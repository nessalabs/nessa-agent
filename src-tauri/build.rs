fn main() {
    println!("cargo:rerun-if-changed=../dist/index.html");
    if std::env::var_os("CARGO_FEATURE_CUSTOM_PROTOCOL").is_some()
        && !std::path::Path::new("../dist/index.html").is_file()
    {
        panic!(
            "embedded frontend missing at dist/index.html; run `pnpm build` before \
             `cargo build -p nessa-app`"
        );
    }
    tauri_build::build()
}
