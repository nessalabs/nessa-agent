//! Native storage for the bundled chat surface. Renderer input never selects a file.
use std::{io::Read, path::PathBuf};

pub struct SurfaceCredential {
    path: Option<PathBuf>,
    stage: String,
}

impl SurfaceCredential {
    pub fn from_environment() -> Self {
        let stage = crate::local_data::process_stage();
        let segment = |value: &str| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        };
        let instance = std::env::var("NESSA_INSTANCE").ok();
        let base = std::env::var("NESSA_DATA_DIR")
            .map(PathBuf::from)
            .ok()
            .or_else(|| {
                std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                    .ok()
                    .map(|home| PathBuf::from(home).join(".nessa"))
            });
        let path = base
            .filter(|base| {
                base.is_absolute() && segment(&stage) && instance.as_deref().is_none_or(segment)
            })
            .map(|base| {
                let mut root = if stage == "prod" {
                    base
                } else {
                    base.join(&stage)
                };
                if let Some(instance) = instance {
                    root = root.join("instances").join(instance);
                }
                root.join("auth/surfaces/nessa-panel.token")
            });
        Self { path, stage }
    }

    fn read(&self, stage: &str) -> Result<String, String> {
        if stage != self.stage {
            return Err("Desktop and gateway stages must match".into());
        }
        let path = self
            .path
            .as_ref()
            .ok_or("Invalid native credential namespace")?;
        let mut file = nessa_local_storage::open(path, nessa_local_storage::OpenMode::Read)
            .map_err(|_| "Chat credential missing or unsafe; run local auth setup")?;
        if file
            .metadata()
            .map_err(|_| "Cannot inspect chat credential")?
            .len()
            > 16385
        {
            return Err("Chat credential is too large".into());
        }
        let mut token = String::new();
        (&mut file)
            .take(16385)
            .read_to_string(&mut token)
            .map_err(|_| "Cannot read chat credential")?;
        let token = token.trim().to_owned();
        if token.is_empty() || token.len() > 16384 {
            return Err("Chat credential is invalid".into());
        }
        Ok(token)
    }
}

#[tauri::command]
pub fn load_surface_credential(
    window: tauri::WebviewWindow,
    storage: tauri::State<'_, SurfaceCredential>,
    stage: String,
) -> Result<String, String> {
    if window.label() != "main" {
        return Err("Only the bundled chat surface can load this credential".into());
    }
    storage.read(&stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn native_storage_checks_stage_and_private_file_permissions() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "nessa-native-{}-{unique}.token",
            std::process::id()
        ));
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(b"fixture-only\n").unwrap();
        let storage = SurfaceCredential {
            path: Some(path.clone()),
            stage: "ci".into(),
        };
        assert_eq!(storage.read("ci").unwrap(), "fixture-only");
        assert!(storage.read("prod").is_err());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(storage.read("ci").is_err());
        }
        drop(file);
        std::fs::remove_file(path).unwrap();
    }
}
