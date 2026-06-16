use std::path::Path;

const KEY_FILE: &str = "identity.nsec";

/// Load the persisted device `nsec` from `<data_dir>/identity.nsec`, generating
/// and persisting a fresh keypair on first launch.
///
/// On Android `data_dir` is the app-private `filesDir`, so the file is not
/// world-readable. The secret never leaves the Rust core (never crosses to the
/// WebView or JS).
pub fn load_or_generate(data_dir: &Path) -> anyhow::Result<String> {
    let path = data_dir.join(KEY_FILE);
    if path.exists() {
        let nsec = std::fs::read_to_string(&path)?.trim().to_string();
        if !nsec.is_empty() {
            return Ok(nsec);
        }
    }
    let id = fips::Identity::generate();
    let nsec = fips::encode_nsec(&id.keypair().secret_key());
    std::fs::write(&path, &nsec)?;
    Ok(nsec)
}
