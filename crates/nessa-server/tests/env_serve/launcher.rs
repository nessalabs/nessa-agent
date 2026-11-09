//! The launcher's search path for a harness that may publish, and where a
//! saved login goes.
use super::*;

#[test]
fn this_build_comes_first_on_the_host_s_search_path() {
    let directory = Path::new("/home/me/.nessa/env/ab12");
    assert_eq!(
        search_path(
            directory,
            Some(std::ffi::OsStr::new("/usr/local/bin:/usr/bin"))
        ),
        Some(OsString::from(
            "/home/me/.nessa/env/ab12:/usr/local/bin:/usr/bin"
        ))
    );
    assert_eq!(
        search_path(directory, None),
        Some(OsString::from("/home/me/.nessa/env/ab12:/bin:/usr/bin"))
    );
    // A directory no search path can hold is not put on one.
    assert_eq!(search_path(Path::new("/odd:dir"), None), None);
}

/// A saved login goes in the variable the agent reads it from, unless the
/// host configures any of that agent's login variables itself: then the
/// host's own wins and the gateway's is not set at all.
#[test]
fn a_saved_login_is_set_only_where_the_host_has_none_of_its_own() {
    let login = Login::new("sk-ant-oat-x".into()).unwrap();
    let none = BTreeMap::new();
    assert_eq!(
        var(AgentId::Claude, Some(&login), &none).as_deref(),
        Some("CLAUDE_CODE_OAUTH_TOKEN")
    );
    assert_eq!(
        var(AgentId::Codex, Some(&login), &none).as_deref(),
        Some("CODEX_API_KEY")
    );
    assert_eq!(
        login_to_set(AgentId::Claude, None, &none, |_| false, None),
        None
    );
    // A saved Anthropic API key goes where Claude Code reads one.
    let key = Login::new("sk-ant-api03-x".into()).unwrap();
    assert_eq!(
        var(AgentId::Claude, Some(&key), &none).as_deref(),
        Some("ANTHROPIC_API_KEY")
    );
    // An API key the host set is the host's login too.
    let own = BTreeMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from("mine"))]);
    assert_eq!(
        login_to_set(AgentId::Claude, Some(&login), &own, |_| false, None),
        None
    );
    // Another agent's variable is not this agent's login.
    assert_eq!(
        var(AgentId::Codex, Some(&login), &own).as_deref(),
        Some("CODEX_API_KEY")
    );
}

fn var(
    agent: AgentId,
    login: Option<&Login>,
    configured: &BTreeMap<OsString, OsString>,
) -> Option<String> {
    let sessions = std::env::temp_dir().join(format!("nessa-codex-test-{}", uuid::Uuid::new_v4()));
    let handed = login_to_set(agent, login, configured, |_| false, Some(&sessions))?;
    let _ = std::fs::remove_dir_all(&sessions);
    Some(handed.into_iter().next().unwrap().0)
}

/// A saved Codex key is the adapter's api-key method, and the key is not
/// written into the user's Codex home.
#[test]
fn a_saved_codex_key_uses_the_api_key_method_in_a_private_home() {
    let login = Login::new("sk-proj-secret".into()).unwrap();
    let sessions = std::env::temp_dir().join(format!("nessa-codex-test-{}", uuid::Uuid::new_v4()));
    let handed = login_to_set(
        AgentId::Codex,
        Some(&login),
        &BTreeMap::new(),
        |_| false,
        Some(&sessions),
    )
    .unwrap();
    let value = |name: &str| {
        handed
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(value("CODEX_API_KEY"), Some("sk-proj-secret"));
    assert_eq!(
        value("DEFAULT_AUTH_REQUEST"),
        Some(r#"{"methodId":"api-key"}"#)
    );
    assert!(!value("DEFAULT_AUTH_REQUEST").unwrap().contains("sk-proj"));
    let home = PathBuf::from(value("CODEX_HOME").unwrap());
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("cli_auth_credentials_store = \"ephemeral\""));
    assert!(!config.contains("sk-proj"));
    assert!(!home.join("auth.json").exists());
    std::fs::remove_dir_all(&sessions).unwrap();
}

/// Codex 0.154 resumes a thread by walking `$CODEX_HOME/sessions` for
/// `rollout-<timestamp>-<thread-id>.jsonl`. A second launch of the same
/// host uses that same home, so the file the first harness left is the
/// one the second one finds. The key stays out of the home and out of
/// the auth request.
#[test]
fn a_second_harness_launch_finds_the_codex_session_the_first_one_stored() {
    let login = Login::new("sk-proj-secret".into()).unwrap();
    let sessions = std::env::temp_dir().join(format!("nessa-codex-test-{}", uuid::Uuid::new_v4()));
    let first = login_to_set(
        AgentId::Codex,
        Some(&login),
        &BTreeMap::new(),
        |_| false,
        Some(&sessions),
    )
    .unwrap();
    let home = PathBuf::from(codex_value(&first, "CODEX_HOME"));
    let session_id = "01936a4e-8c2a-7b2e-9c1d-0a1b2c3d4e5f";
    let rollout = home
        .join("sessions/2026/11/09")
        .join(format!("rollout-2026-11-09T10-00-00-{session_id}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, "{}\n").unwrap();
    let mut config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    config = config.replace(
        "cli_auth_credentials_store = \"ephemeral\"",
        "cli_auth_credentials_store = \"file\"",
    );
    config.push_str("model = \"gpt\"\n");
    std::fs::write(home.join("config.toml"), &config).unwrap();

    let second = login_to_set(
        AgentId::Codex,
        Some(&login),
        &BTreeMap::new(),
        |_| false,
        Some(&sessions),
    )
    .unwrap();
    assert_eq!(codex_value(&second, "CODEX_HOME"), home.to_str().unwrap());
    assert_eq!(
        codex_value(&second, "DEFAULT_AUTH_REQUEST"),
        r#"{"methodId":"api-key"}"#
    );
    assert!(!codex_value(&second, "DEFAULT_AUTH_REQUEST").contains("sk-proj"));
    let found = codex_thread_rollout(&home, session_id);
    assert_eq!(found.as_deref(), Some(rollout.as_path()));
    let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
    assert!(config.contains("cli_auth_credentials_store = \"ephemeral\""));
    assert!(config.contains("model = \"gpt\""));
    assert!(!config.contains("sk-proj"));
    assert!(!home.join("auth.json").exists());
    std::fs::remove_dir_all(&sessions).unwrap();
}

fn codex_value(handed: &[(String, String)], name: &str) -> String {
    handed
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .unwrap()
}

/// The file Codex 0.154's `find_thread_path_by_id_str` would select: a
/// `rollout-YYYY-MM-DDThh-mm-ss-<thread-id>.jsonl` under `sessions/`.
fn codex_thread_rollout(home: &Path, session_id: &str) -> Option<PathBuf> {
    let mut stack = vec![home.join("sessions")];
    let mut found = None;
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(core) = name
                .strip_prefix("rollout-")
                .and_then(|name| name.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Some(ids) = core.get(20..) else {
                continue;
            };
            if core.as_bytes().get(19) != Some(&b'-') {
                continue;
            }
            let thread_id = ids.split('_').next().unwrap_or(ids);
            if thread_id == session_id {
                found = Some(path);
            }
        }
    }
    found
}

/// A sign-in the agent's own login command wrote on the host wins too:
/// Claude Code's `.credentials.json`, Codex's `auth.json`, where the
/// harness's environment points them.
#[test]
fn a_host_signed_in_with_the_agent_s_own_file_keeps_it() {
    let login = Login::new("sk-ant-oat-x".into()).unwrap();
    let home = BTreeMap::from([(OsString::from("HOME"), OsString::from("/home/me"))]);
    assert_eq!(
        own_sign_in_files(AgentId::Claude, &home),
        [PathBuf::from("/home/me/.claude/.credentials.json")]
    );
    // OpenCode's auth.json is not a sign-in this launch's harness reads.
    assert!(own_sign_in_files(AgentId::Opencode, &home).is_empty());
    assert_eq!(
        var(AgentId::Opencode, Some(&login), &home).as_deref(),
        Some("OPENCODE_API_KEY")
    );
    let own_key = BTreeMap::from([(OsString::from("OPENCODE_API_KEY"), OsString::from("mine"))]);
    assert_eq!(
        login_to_set(AgentId::Opencode, Some(&login), &own_key, |_| true, None),
        None
    );
    let pointed = BTreeMap::from([
        (OsString::from("HOME"), OsString::from("/home/me")),
        (OsString::from("CODEX_HOME"), OsString::from("/srv/codex")),
    ]);
    assert_eq!(
        own_sign_in_files(AgentId::Codex, &pointed),
        [PathBuf::from("/srv/codex/auth.json")]
    );
    let signed_in = |path: &Path| path == Path::new("/srv/codex/auth.json");
    let absent = std::env::temp_dir().join(format!("nessa-codex-absent-{}", uuid::Uuid::new_v4()));
    assert_eq!(
        login_to_set(
            AgentId::Codex,
            Some(&login),
            &pointed,
            signed_in,
            Some(&absent),
        ),
        None
    );
    assert!(!absent.exists());
    // Claude's file is somewhere else: not signed in, so it is set.
    assert_eq!(
        var(AgentId::Claude, Some(&login), &pointed).as_deref(),
        Some("CLAUDE_CODE_OAUTH_TOKEN")
    );
    // No HOME and nothing pointed: nowhere to look.
    assert!(own_sign_in_files(AgentId::Claude, &BTreeMap::new()).is_empty());
}

/// A Codex home whose `config.toml` already holds tables, or spells the
/// setting as a quoted key, gets the credential store set by TOML key: one
/// root key, read back as ephemeral, everything else kept. A config that is
/// not TOML is left alone and the key is not handed.
#[test]
fn the_ephemeral_store_is_set_by_toml_key_in_an_existing_config() {
    let login = Login::new("sk-proj-secret".into()).unwrap();
    for existing in [
        "model = \"gpt\"\n\n[projects.\"/workspace\"]\ntrust_level = \"trusted\"\n",
        "\"cli_auth_credentials_store\" = \"file\"\nmodel = \"gpt\"\n\n[projects.\"/workspace\"]\ntrust_level = \"trusted\"\n",
        "model = \"gpt\"\n[projects.\"/workspace\"]\ntrust_level = \"trusted\"\ncli_auth_credentials_store = \"file\"\n",
    ] {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("codex");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.toml"), existing).unwrap();
        for _ in 0..2 {
            login_to_set(AgentId::Codex, Some(&login), &BTreeMap::new(), |_| false, Some(&home))
                .unwrap();
            let config = std::fs::read_to_string(home.join("config.toml")).unwrap();
            let document: toml_edit::DocumentMut = config.parse().unwrap();
            assert_eq!(
                document["cli_auth_credentials_store"].as_str(),
                Some("ephemeral"),
                "{config}"
            );
            assert_eq!(document["model"].as_str(), Some("gpt"));
            assert_eq!(
                document["projects"]["/workspace"]["trust_level"].as_str(),
                Some("trusted")
            );
        }
    }
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("codex");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "not = [toml").unwrap();
    assert_eq!(
        login_to_set(
            AgentId::Codex,
            Some(&login),
            &BTreeMap::new(),
            |_| false,
            Some(&home)
        ),
        None
    );
    assert_eq!(
        std::fs::read_to_string(home.join("config.toml")).unwrap(),
        "not = [toml"
    );
}

/// An exported but empty login variable is no sign-in: the saved login is
/// still handed.
#[test]
fn an_empty_login_variable_is_not_the_host_s_own_sign_in() {
    let login = Login::new("sk-ant-oat-x".into()).unwrap();
    let empty = BTreeMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::new())]);
    assert_eq!(
        var(AgentId::Claude, Some(&login), &empty).as_deref(),
        Some("CLAUDE_CODE_OAUTH_TOKEN")
    );
    let set = BTreeMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from("mine"))]);
    assert_eq!(var(AgentId::Claude, Some(&login), &set), None);
}
