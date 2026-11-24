fn main() {
    println!("cargo:rerun-if-changed=../../../packaging/windows/better-kumo.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../../packaging/windows/better-kumo.ico")
            .set("ProductName", "Better Kumo")
            .set("FileDescription", "Better Kumo");
        res.compile().expect("Windows resources");
    }
}
