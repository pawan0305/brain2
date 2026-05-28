fn main() {
    // For `tauri dev` we run the unbundled Rust binary directly. macOS will
    // honour usage-description strings embedded into the binary via the
    // __TEXT,__info_plist section, so screen-capture and mic permission
    // prompts work without a full .app bundle.
    #[cfg(target_os = "macos")]
    {
        let plist = std::path::Path::new("Info.plist");
        if plist.exists() {
            let absolute = std::fs::canonicalize(plist).expect("canonicalize Info.plist");
            println!(
                "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
                absolute.display()
            );
            println!("cargo:rerun-if-changed=Info.plist");
        }
    }
    tauri_build::build();
}
