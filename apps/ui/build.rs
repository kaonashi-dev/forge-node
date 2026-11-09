use std::fs;

fn main() {
    println!("cargo:rerun-if-changed=../../Cargo.toml");
    let manifest = fs::read_to_string("../../Cargo.toml").unwrap_or_default();
    let mut in_workspace_package = false;
    let mut version = None;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_workspace_package = line == "[workspace.package]";
            continue;
        }
        if in_workspace_package && let Some(value) = line.strip_prefix("version = ") {
            version = value.trim().trim_matches('"').to_string().into();
            break;
        }
    }
    let version = version.unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    println!("cargo:rustc-env=FORGE_NODE_VERSION={version}");
}
