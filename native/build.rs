//! Gives the Windows program the DataHaven icon and its name in Explorer. Does nothing elsewhere.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/icon.ico");
        resource.set("ProductName", "DataHaven");
        resource.set("FileDescription", "DataHaven backups");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=Could not add the Windows icon: {error}");
        }
    }
}
