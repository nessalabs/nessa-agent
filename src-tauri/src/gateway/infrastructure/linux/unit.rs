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

pub(super) fn rendered_agent_path(bytes: &[u8]) -> Result<SearchPath, String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "The systemd gateway definition is not UTF-8".to_string())?;
    let mut commands = text
        .lines()
        .filter_map(|line| line.strip_prefix("ExecStart="));
    let command = commands
        .next()
        .ok_or_else(|| "The systemd gateway definition has no ExecStart".to_string())?;
    if commands.next().is_some() {
        return Err("The systemd gateway definition has multiple ExecStart entries".into());
    }
    let arguments = parse_rendered_command(command)?;
    if arguments.first().map(String::as_str) != Some("/usr/bin/env")
        || arguments.get(1).map(String::as_str) != Some("-i")
    {
        return Err("The systemd gateway definition has an unexpected command".into());
    }
    let paths = arguments
        .iter()
        .filter_map(|argument| argument.strip_prefix("NESSA_AGENT_PATH="))
        .collect::<Vec<_>>();
    let [path] = paths.as_slice() else {
        return Err("The systemd gateway definition has no unique agent path".into());
    };
    SearchPath::parse(path).map_err(|error| error.to_string())
}

fn parse_rendered_command(command: &str) -> Result<Vec<String>, String> {
    let bytes = command.as_bytes();
    let mut cursor = 0;
    let mut arguments = Vec::new();
    if bytes.first() == Some(&b':') {
        cursor += 1;
    }
    while cursor < bytes.len() {
        if bytes[cursor] != b'"' {
            return Err("The systemd gateway command is not canonically quoted".into());
        }
        cursor += 1;
        let mut value = Vec::new();
        while cursor < bytes.len() && bytes[cursor] != b'"' {
            match bytes[cursor] {
                b'\\' => {
                    cursor += 1;
                    let escaped = *bytes.get(cursor).ok_or_else(|| {
                        "The systemd gateway command has an incomplete escape".to_string()
                    })?;
                    value.push(match escaped {
                        b'n' => b'\n',
                        b'r' => b'\r',
                        b't' => b'\t',
                        b'\\' | b'"' => escaped,
                        _ => return Err("The systemd gateway command has an unknown escape".into()),
                    });
                }
                b'%' if bytes.get(cursor + 1) == Some(&b'%') => {
                    value.push(b'%');
                    cursor += 1;
                }
                byte => value.push(byte),
            }
            cursor += 1;
        }
        if bytes.get(cursor) != Some(&b'"') {
            return Err("The systemd gateway command has an unterminated value".into());
        }
        cursor += 1;
        arguments.push(
            String::from_utf8(value)
                .map_err(|_| "The systemd gateway command value is not UTF-8".to_string())?,
        );
        if cursor == bytes.len() {
            break;
        }
        if bytes[cursor] != b' ' {
            return Err("The systemd gateway command has unexpected separators".into());
        }
        cursor += 1;
    }
    Ok(arguments)
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

    #[test]
    fn recovers_the_agent_path_only_from_the_canonical_rendered_command() {
        let unit = unit_name("prod", None).unwrap();
        let expected = SearchPath::parse("/usr/bin:/opt/with space:/percent%bin").unwrap();
        let rendered = render(UnitDefinition {
            unit: &unit,
            runtime: Path::new("/runtime"),
            configuration: &configuration(),
            data: Path::new("/data"),
            home: Path::new("/home/me"),
            agent_path: &expected,
            fingerprint: &"a".repeat(64),
            generation: &"b".repeat(64),
        })
        .unwrap();
        assert_eq!(rendered_agent_path(&rendered.bytes).unwrap(), expected);

        let mut contradictory = rendered.bytes;
        contradictory.extend_from_slice(b"ExecStart=:\"/usr/bin/env\"\n");
        assert!(rendered_agent_path(&contradictory).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_analyze_accepts_the_rendered_user_unit() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let runtime = root.join("runtime");
        let data = root.join("data");
        fs::create_dir(&runtime).unwrap();
        fs::create_dir(&data).unwrap();
        let executable = runtime.join("nessa");
        fs::write(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let unit = unit_name("prod", None).unwrap();
        let rendered = render(UnitDefinition {
            unit: &unit,
            runtime: &runtime,
            configuration: &ServiceConfiguration::new(
                "prod".into(),
                data.clone(),
                None,
                7420,
                None,
            )
            .unwrap(),
            data: &data,
            home: &root,
            agent_path: &SearchPath::parse("/usr/bin").unwrap(),
            fingerprint: &"a".repeat(64),
            generation: &"b".repeat(64),
        })
        .unwrap();
        let path = root.join(unit.as_str());
        fs::write(&path, rendered.bytes).unwrap();
        let output = Command::new("systemd-analyze")
            .args(["--user", "verify"])
            .arg(&path)
            .output()
            .expect("Ubuntu desktop CI must provide systemd-analyze");
        assert!(
            output.status.success(),
            "systemd-analyze rejected the rendered unit: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
