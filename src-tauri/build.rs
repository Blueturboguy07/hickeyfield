fn main() {
    publik_app_token();
    tauri_build::build()
}

/// The publik app token the module compiles in (src/publik.rs): `PUBLIK_APP_TOKEN` at build
/// time, else `publik-app-token.txt` next to Cargo.toml, else empty. An empty token makes the
/// built app say "This build has no publik app token. Ask the developer for a new build."
fn publik_app_token() {
    println!("cargo:rerun-if-env-changed=PUBLIK_APP_TOKEN");
    println!("cargo:rerun-if-changed=publik-app-token.txt");
    let token = std::env::var("PUBLIK_APP_TOKEN")
        .ok()
        .filter(|t| !t.trim().is_empty())
        .or_else(|| std::fs::read_to_string("publik-app-token.txt").ok())
        .unwrap_or_default();
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo"));
    std::fs::write(out.join("publik-app-token.txt"), token.trim())
        .expect("write the publik app token into OUT_DIR");
}
