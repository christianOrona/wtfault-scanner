fn main() {
    // The release build embeds the compiled UI so the app can serve it from the
    // same origin as the API. `include_dir!` needs the directory to exist at
    // compile time, and a fresh clone running `tauri dev` has never produced
    // one — so make sure there is at least an empty directory to embed. In dev
    // the window loads from the Vite server anyway and this is never read.
    let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist");
    if !dist.exists() {
        let _ = std::fs::create_dir_all(&dist);
    }

    tauri_build::build();
}
