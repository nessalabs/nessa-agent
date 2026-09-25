use crate::gateway::domain::value_objects::SystemdUnitName;
use std::{
    ffi::OsString,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct LinuxGatewayPaths {
    pub config_root: PathBuf,
    pub data_root: PathBuf,
    pub unit_root: PathBuf,
    pub unit_file: PathBuf,
    pub wants_directory: PathBuf,
    pub wants_link: PathBuf,
    pub runtime_root: PathBuf,
}

impl LinuxGatewayPaths {
    pub fn new(
        home: &Path,
        config_home: Option<OsString>,
        data_home: Option<OsString>,
        unit: &SystemdUnitName,
    ) -> Result<Self, String> {
        let home = normalized_absolute(home)
            .then(|| home.to_path_buf())
            .ok_or_else(|| {
                "The account home required for the gateway is not absolute and normalized"
                    .to_string()
            })?;
        let config = xdg_root(config_home, &home, ".config", "XDG_CONFIG_HOME")?;
        let data = xdg_root(data_home, &home, ".local/share", "XDG_DATA_HOME")?;
        let unit_root = config.join("systemd/user");
        let wants_directory = unit_root.join("default.target.wants");
        Ok(Self {
            config_root: config.clone(),
            data_root: data.clone(),
            unit_file: unit_root.join(unit.as_str()),
            wants_link: wants_directory.join(unit.as_str()),
            runtime_root: data.join("nessa/gateway-runtimes").join(unit.as_str()),
            unit_root,
            wants_directory,
        })
    }
}

fn xdg_root(
    configured: Option<OsString>,
    home: &Path,
    fallback: &str,
    name: &str,
) -> Result<PathBuf, String> {
    match configured {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(value);
            normalized_absolute(&path)
                .then_some(path)
                .ok_or_else(|| format!("{name} must be absolute and normalized"))
        }
        _ => Ok(home.join(fallback)),
    }
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_owned_unit_link_and_runtime_roots_from_desktop_xdg() {
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        let paths = LinuxGatewayPaths::new(
            Path::new("/home/me"),
            Some("/cfg".into()),
            Some("/data".into()),
            &unit,
        )
        .unwrap();
        assert_eq!(
            paths.unit_file,
            Path::new("/cfg/systemd/user/nessa-gateway-prod.service")
        );
        assert_eq!(
            paths.wants_link,
            Path::new("/cfg/systemd/user/default.target.wants/nessa-gateway-prod.service")
        );
        assert_eq!(
            paths.runtime_root,
            Path::new("/data/nessa/gateway-runtimes/nessa-gateway-prod.service")
        );
    }

    #[test]
    fn refuses_relative_xdg_roots() {
        let unit = SystemdUnitName::parse("nessa-gateway-prod.service".into()).unwrap();
        assert!(LinuxGatewayPaths::new(
            Path::new("/home/me"),
            Some("relative".into()),
            None,
            &unit,
        )
        .is_err());
    }
}
