#[path = "src/icon_art.rs"]
mod icon_art;
fn main() {
    println!("cargo:rerun-if-changed=product.json");
    println!("cargo:rerun-if-changed=src/icon_art.rs");
    let product: serde_json::Value =
        serde_json::from_slice(&std::fs::read("product.json").expect("Read product.json"))
            .expect("Parse product.json");
    for (key, variable) in [
        ("name", "FINDER_PRODUCT_NAME"),
        ("publisher", "FINDER_PUBLISHER"),
        ("id", "FINDER_APP_ID"),
        ("install_marker", "FINDER_INSTALL_MARKER"),
    ] {
        let value = product[key]
            .as_str()
            .expect("Product identity must be a string");
        assert!(
            !value.is_empty()
                && value != "."
                && value != ".."
                && value
                    .chars()
                    .all(|c| !c.is_control() && c != '/' && c != char::from(92)),
            "Invalid product identity"
        );
        println!("cargo:rustc-env={variable}={value}");
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let icon =
            std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("finder.ico");
        std::fs::write(&icon, ico()).expect("Write Finder icon");
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon(icon.to_str().unwrap())
            .set("ProductName", product["name"].as_str().unwrap())
            .set("FileDescription", "Finder file search")
            .set("CompanyName", product["publisher"].as_str().unwrap())
            .set(
                "LegalCopyright",
                &format!("Copyright (c) {}", product["publisher"].as_str().unwrap()),
            );
        resource
            .compile()
            .expect("Compile Finder Windows resources");
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
    }
}

fn ico() -> Vec<u8> {
    let sizes = [16u32, 32, 48, 256];
    let images: Vec<_> = sizes.iter().map(|&size| icon_art::dib(size)).collect();
    let mut bytes = vec![0, 0, 1, 0, sizes.len() as u8, 0];
    let mut offset = (6 + 16 * sizes.len()) as u32;
    for (&size, image) in sizes.iter().zip(&images) {
        bytes.extend_from_slice(&[size as u8, size as u8, 0, 0, 1, 0, 32, 0]);
        bytes.extend_from_slice(&(image.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        offset += image.len() as u32;
    }
    for image in images {
        bytes.extend(image);
    }
    bytes
}
