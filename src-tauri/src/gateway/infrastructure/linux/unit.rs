use crate::gateway::domain::value_objects::{SearchPath, ServiceConfiguration, SystemdUnitName};
use std::path::Path;

pub(super) fn unit_name(stage: &str, instance: Option<&str>) -> Result<SystemdUnitName, String> {
    let suffix = instance.map_or_else(String::new, |value| format!("-{value}"));
    SystemdUnitName::parse(format!("nessa-gateway-{stage}{suffix}.service"))
        .map_err(|error| error.to_string())
}

pub(super) struct RenderedUnit {
    pub bytes: Vec<u8>,
    pub arguments: Vec<String>,
    pub working_directory: String,
}

pub(super) struct UnitDefinition<'a> {
    pub unit: &'a SystemdUnitName,
    pub runtime: &'a Path,
    pub configuration: &'a ServiceConfiguration,
    pub data: &'a Path,
    pub home: &'a Path,
    pub agent_path: &'a SearchPath,
    pub fingerprint: &'a str,
    pub generation: &'a str,
}

pub(super) fn render(input: UnitDefinition<'_>) -> Result<RenderedUnit, String> {
    let UnitDefinition {
        unit,
        runtime,
        configuration,
        data,
        home,
        agent_path,
        fingerprint,
        generation,
    } = input;
    let executable = runtime.join("nessa");
    let working = data;
    let mut environment = vec![
        ("HOME", path(home)?),
        ("NESSA_STAGE", configuration.stage().to_owned()),
        ("NESSA_HOST", "127.0.0.1".into()),
        ("NESSA_PORT", configuration.port().to_string()),
        ("NESSA_DATA_DIR", path(data)?),
        ("NESSA_AGENT_PATH", agent_path.as_str().to_owned()),
        ("NESSA_RUNTIME_FINGERPRINT", fingerprint.to_owned()),
        ("NESSA_SERVICE_GENERATION", generation.to_owned()),
    ];
    if let Some(instance) = configuration.instance() {
        environment.push(("NESSA_INSTANCE", instance.to_owned()));
    }
    if let Some(directory) = configuration.claude_config_directory() {
        environment.push(("CLAUDE_CONFIG_DIR", path(directory)?));
    }
    let mut arguments = vec!["/usr/bin/env".to_owned(), "-i".to_owned()];
    arguments.extend(
        environment
            .into_iter()
            .map(|(key, value)| format!("{key}={value}")),
    );
    arguments.extend([
        path(&executable)?,
        "server".into(),
        "--desktop-runtime".into(),
        path(runtime)?,
    ]);
    let command = arguments
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let escaped = quote(value)?;
            Ok(if index == 0 {
                format!(":{escaped}")
            } else {
                escaped
            })
        })
        .collect::<Result<Vec<_>, String>>()?
        .join(" ");
    let description = quote(&format!("Nessa gateway ({})", unit.as_str()))?;
    let working_directory = path(working)?;
    let bytes = format!(
        "[Unit]\nDescription={description}\nAfter=network.target\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart={command}\nRestart=on-failure\nRestartSec=5s\nTimeoutStopSec=30s\n\n[Install]\nWantedBy=default.target\n",
        quote(&working_directory)?,
    )
    .into_bytes();
    Ok(RenderedUnit {
        bytes,
        arguments,
        working_directory,
    })
}

fn path(value: &Path) -> Result<String, String> {
    value
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "A systemd gateway path is not UTF-8".into())
}

fn quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("A systemd unit value contains NUL".into());
    }
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '%' => escaped.push_str("%%"),
            character if character.is_control() => {
                return Err("A systemd unit value contains an unsupported control character".into())
            }
            character => escaped.push(character),
        }
    }
    escaped.push('"');
    Ok(escaped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn configuration() -> ServiceConfiguration {
        ServiceConfiguration::new("prod".into(), PathBuf::from("/data"), None, 7420, None).unwrap()
    }

    #[test]
    fn renders_no_shell_clean_environment_and_percent_escaping() {
        let unit = unit_name("prod", None).unwrap();
        let rendered = String::from_utf8(
            render(UnitDefinition {
                unit: &unit,
                runtime: Path::new("/runtime%one"),
                configuration: &configuration(),
                data: Path::new("/data"),
                home: Path::new("/home/me"),
                agent_path: &SearchPath::parse("/usr/bin:/opt/tools").unwrap(),
                fingerprint: &"a".repeat(64),
                generation: &"b".repeat(64),
            })
            .unwrap()
            .bytes,
        )
        .unwrap();
        assert!(rendered.contains("ExecStart=:\"/usr/bin/env\" \"-i\""));
        assert!(rendered.contains("/runtime%%one/nessa"));
        assert!(!rendered.contains("sh -c"));
        assert!(rendered.contains("WantedBy=default.target"));
    }

    #[test]
    fn rejects_control_characters_before_publication() {
        assert!(quote("line\u{7}").is_err());
        assert_eq!(quote("100%"), Ok("\"100%%\"".into()));
    }
}
