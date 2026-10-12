fn main() {
    // Writing to a physical drive needs administrator rights: ask Windows for them when the program starts.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("windows/toastrack-gui.rc", embed_resource::NONE);
    }
    println!("cargo:rerun-if-changed=windows");
}
