fn main() {
    println!("cargo:rerun-if-changed=build/icon.ico");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("build/icon.ico");
        resource.set("ProductName", "SpotifySync");
        resource.set(
            "FileDescription",
            "SpotifySync - Spotify and Discord status",
        );
        resource
            .compile()
            .expect("Failed to embed the SpotifySync Windows application icon");
    }
}
