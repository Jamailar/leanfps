fn main() {
    println!("cargo:rerun-if-changed=res/leanfps.rc");
    println!("cargo:rerun-if-changed=res/leanfps.manifest");
    println!("cargo:rerun-if-changed=res/leanfps.ico");
    println!("cargo:rerun-if-changed=res/tray-idle.ico");
    println!("cargo:rerun-if-changed=res/tray-active.ico");
    #[cfg(windows)]
    {}
    // Cross-compiling from Linux: cfg(windows) is false for the build script, so check the target.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("res/leanfps.rc", embed_resource::NONE).manifest_required().unwrap();
    }
}
