use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

fn main() {
    let build_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let display_name = read_display_name_from_manifest().unwrap_or_else(|| {
        std::env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "biscotti".to_owned())
    });

    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rustc-env=BISCOTTI_APP_DISPLAY_NAME={display_name}");
    println!("cargo:rustc-env=BISCOTTI_BUILD_UNIX_SECONDS={build_unix_seconds}");
}

fn read_display_name_from_manifest() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").ok()?;
    let manifest_path = Path::new(&manifest_dir).join("Cargo.toml");
    let manifest = fs::read_to_string(manifest_path).ok()?;
    let mut in_biscotti_metadata = false;

    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_biscotti_metadata = trimmed == "[package.metadata.biscotti]";
            continue;
        }

        if !in_biscotti_metadata {
            continue;
        }

        let Some(value) = trimmed.strip_prefix("display-name") else {
            continue;
        };
        let Some(value) = value.split_once('=').map(|(_, value)| value.trim()) else {
            continue;
        };

        return value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
            .map(ToOwned::to_owned);
    }

    None
}
