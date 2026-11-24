fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // `ort-sys` links static Objective-C++ objects (CoreML version checks)
        // that reference `___isPlatformVersionAtLeast` from clang's runtime.
        // In release/LTO builds rustc does not pass `libclang_rt.osx.a` automatically.
        if let Ok(output) = std::process::Command::new("clang")
            .arg("--print-file-name=libclang_rt.osx.a")
            .output()
            && output.status.success()
        {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if let Some(dir) = std::path::Path::new(&path).parent()
                && !dir.as_os_str().is_empty()
            {
                println!("cargo:rustc-link-search=native={}", dir.display());
                println!("cargo:rustc-link-lib=static=clang_rt.osx");
            }
        }
    }
    tauri_build::build()
}
