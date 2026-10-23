use serde::{Deserialize, Serialize};
use std::{io, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kubeconfig_path: PathBuf,
    pub context: String,
    pub namespace: String,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub last_cluster_id: Option<String>,
    pub clusters: Vec<Connection>,
}

pub fn path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "Kube")
        .map(|dirs| dirs.config_dir().join("settings.json"))
}

pub fn load() -> Result<Settings, &'static str> {
    let Some(path) = path() else {
        return Err("Could not locate the settings directory.");
    };
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| "Settings are invalid. Existing settings will not be overwritten."),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Could not read settings. Check file permissions."),
    }
}

pub fn save(settings: &Settings) -> Result<(), &'static str> {
    let path = path().ok_or("Could not locate the settings directory.")?;
    save_at(settings, &path)
        .map_err(|_| "Could not save settings. Check disk space and permissions.")
}

fn save_at(settings: &Settings, path: &std::path::Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(settings)?;
    // A single writer runs off the UI thread. Never store kubeconfig contents or logs.
    let temporary = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_roundtrip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut settings = Settings::default();
        save_at(&settings, &path).unwrap();
        settings.clusters.push(Connection {
            id: "1".into(),
            name: "Тест".into(),
            kubeconfig_path: PathBuf::from("C:\\test\\config"),
            context: "ctx".into(),
            namespace: "ns".into(),
        });
        settings.last_cluster_id = Some("1".into());
        save_at(&settings, &path).unwrap();
        let restored: Settings = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(settings, restored);
    }
}
