use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    let base = dirs::config_dir()
        .or_else(|| dirs::home_dir())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".hakimi-hub")
}

pub fn ca_cert_path() -> PathBuf {
    data_dir().join("ca.crt.pem")
}

pub fn ca_key_path() -> PathBuf {
    data_dir().join("ca.key.pem")
}

pub fn cert_cache_dir() -> PathBuf {
    data_dir().join("certs")
}

pub fn pid_file_path() -> PathBuf {
    data_dir().join("hakimi-hub.pid")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

pub fn ensure_data_dir() -> std::io::Result<()> {
    let dir = data_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir)?;
    }
    Ok(())
}

pub fn ensure_all_dirs() -> std::io::Result<()> {
    ensure_data_dir()?;
    let dirs = [cert_cache_dir(), log_dir(), cache_dir()];
    for dir in dirs {
        if !dir.exists() {
            std::fs::create_dir_all(&dir)?;
        }
    }
    Ok(())
}
