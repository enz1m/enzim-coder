use std::process::Command;

fn main() {
    let status = Command::new("glib-compile-resources")
        .args([
            "--sourcedir=.",
            "--target=resources.gresource",
            "icons.gresource.xml",
        ])
        .status()
        .expect("Failed to compile gresource");

    if !status.success() {
        panic!("glib-compile-resources failed");
    }

    println!("cargo:rerun-if-changed=icons.gresource.xml");
    println!("cargo:rerun-if-changed=icons/");
}
