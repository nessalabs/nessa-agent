//! Native storage for the bundled chat surface. Renderer input never selects a file.
use crate::gateway::application::Gateway;
use std::{io::Read, path::PathBuf};
use tauri::Manager;

pub struct SurfaceCredential {
    root: Option<PathBuf>,
    relative: PathBuf,
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
        let root = base.filter(|base| {
            base.is_absolute() && segment(&stage) && instance.as_deref().is_none_or(segment)
        });
        let mut relative = PathBuf::new();
        if stage != "prod" {
            relative.push(&stage);
        }
        if let Some(instance) = instance {
            relative.push("instances");
            relative.push(instance);
        }
        relative.push("auth/surfaces/nessa-panel.token");
        Self {
            root,
            relative,
            stage,
        }
    }

    fn read(&self, stage: &str) -> Result<String, String> {
        if stage != self.stage {
            return Err("Desktop and gateway stages must match".into());
        }
        let root = self
            .root
            .as_ref()
            .ok_or("Invalid native credential namespace")?;
        let mut file = nessa_local_storage::open_beneath(
            root,
            &self.relative,
            nessa_local_storage::OpenMode::ReadNonblocking,
        )
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
pub async fn load_surface_credential(
    window: tauri::WebviewWindow,
    storage: tauri::State<'_, SurfaceCredential>,
    stage: String,
) -> Result<String, String> {
    if window.label() != "main" {
        return Err("Only the bundled chat surface can load this credential".into());
    }
    if let Some(gateway) = window.app_handle().try_state::<Gateway>() {
        gateway
            .wait_ready()
            .await
            .map_err(|error| error.to_string())?;
    }
    storage.read(&stage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, io::Write};

    fn temporary_directory(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "nessa-native-{name}-{}-{unique}",
            std::process::id()
        ));
        nessa_local_storage::create_directory(&root).unwrap();
        root
    }

    #[test]
    fn native_storage_checks_stage_and_private_file_permissions() {
        let root = temporary_directory("regular");
        let path = root.join("nessa-panel.token");
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(b"fixture-only\n").unwrap();
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "nessa-panel.token".into(),
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
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_storage_rejects_a_fifo_without_waiting_for_a_writer() {
        let root = temporary_directory("fifo");
        let path = root.join("nessa-panel.token");
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "nessa-panel.token".into(),
            stage: "ci".into(),
        };
        assert!(storage.read("ci").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn native_storage_rejects_an_intermediate_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_directory("intermediate-link");
        let outside = temporary_directory("outside");
        let mut token = nessa_local_storage::open(
            &outside.join("nessa-panel.token"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        token.write_all(b"redirected-token").unwrap();
        drop(token);
        symlink(&outside, root.join("auth")).unwrap();
        let storage = SurfaceCredential {
            root: Some(root.clone()),
            relative: "auth/nessa-panel.token".into(),
            stage: "ci".into(),
        };
        assert!(storage.read("ci").is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
