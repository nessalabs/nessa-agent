//! Which cloud new thread VMs are made on, in one private file. Missing
//! means Boat, the cloud a gateway offered before it offered a choice.
use crate::workspaces::{
    application::CloudChoice,
    domain::{Cloud, Failure},
};
use async_trait::async_trait;
use std::{io::Write, os::unix::fs::OpenOptionsExt, path::PathBuf};

pub struct FileCloudChoice(pub PathBuf);
#[async_trait]
impl CloudChoice for FileCloudChoice {
    async fn read(&self) -> Result<Cloud, Failure> {
        let path = self.0.clone();
        tokio::task::spawn_blocking(move || match std::fs::read(&path) {
            Ok(bytes) => match bytes.as_slice() {
                b"boat" => Ok(Cloud::Boat),
                b"agent37" => Ok(Cloud::Agent37),
                _ => Err(Failure::Storage),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Cloud::Boat),
            Err(_) => Err(Failure::Storage),
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, cloud: Cloud) -> Result<(), Failure> {
        let path = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let next = path.with_extension("next");
            let _ = std::fs::remove_file(&next);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&next)?;
            file.write_all(cloud.as_str().as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&next, &path)?;
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
            tracing::info!(
                target: "nessa_server::workspaces",
                cloud = cloud.as_str(),
                "new thread VMs go to this cloud"
            );
            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(|_| Failure::Storage)?
        .map_err(|_| Failure::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_choice_is_boat_until_another_is_saved_and_survives_a_restart() {
        let place = tempfile::tempdir().unwrap();
        let path = place.path().join("cloud");
        assert_eq!(FileCloudChoice(path.clone()).read().await, Ok(Cloud::Boat));
        FileCloudChoice(path.clone())
            .save(Cloud::Agent37)
            .await
            .unwrap();
        assert_eq!(
            FileCloudChoice(path.clone()).read().await,
            Ok(Cloud::Agent37)
        );
        std::fs::write(&path, "elsewhere").unwrap();
        assert_eq!(FileCloudChoice(path).read().await, Err(Failure::Storage));
    }
}
