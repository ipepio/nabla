// Tauri's context macro wants the build script's environment (it embeds the config
// and the frontend assets at compile time), so the probe carries the standard one.
fn main() {
    tauri_build::build()
}
