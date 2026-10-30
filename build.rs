fn main() {
    #[cfg(target_os = "windows")]
    {
        println!("cargo:rerun-if-changed=resources/strata.rc");
        println!("cargo:rerun-if-changed=assets/brand/strata.ico");
        embed_resource::compile("resources/strata.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("Cannot embed the 101 Downloader icon");
    }
}
