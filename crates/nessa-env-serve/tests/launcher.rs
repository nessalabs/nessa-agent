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
        login_to_set(None, AgentId::Claude, None, &none, |_| false, None),
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
        login_to_set(None, AgentId::Claude, Some(&login), &own, |_| false, None),
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
    let handed = login_to_set(None, agent, login, configured, |_| false, Some(&sessions))?;
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
        None,
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
        None,
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
        None,
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

/// A ChatGPT access token is handed in the ChatGPT provider's variable,
/// in a home of its own beside the key's whose default provider reads it,
/// with no auth request; a thread the key's home stored is found from it.
#[cfg(unix)]
#[test]
fn a_chatgpt_access_token_uses_the_chatgpt_provider_in_its_own_home() {
    let token = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl";
    let login = Login::new(token.into()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("codex");
    // The key's home stored a thread first.
    let key = Login::new("sk-proj-secret".into()).unwrap();
    let keyed = login_to_set(
        None,
        AgentId::Codex,
        Some(&key),
        &BTreeMap::new(),
        |_| false,
        Some(&sessions),
    )
    .unwrap();
    let key_home = PathBuf::from(codex_value(&keyed, "CODEX_HOME"));
    let session_id = "01936a4e-8c2a-7b2e-9c1d-0a1b2c3d4e5f";
    let rollout = key_home
        .join("sessions/2026/11/09")
        .join(format!("rollout-2026-11-09T10-00-00-{session_id}.jsonl"));
    std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    std::fs::write(&rollout, "{}\n").unwrap();

    // Outside a lease, nothing could renew it: not handed.
    assert_eq!(
        login_to_set(
            None,
            AgentId::Codex,
            Some(&login),
            &BTreeMap::new(),
            |_| false,
            Some(&sessions)
        ),
        None
    );
    for _ in 0..2 {
        let handed = login_to_set(
            Some("lease-1"),
            AgentId::Codex,
            Some(&login),
            &BTreeMap::new(),
            |_| false,
            Some(&sessions),
        )
        .unwrap();
        let names: Vec<&str> = handed.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["NESSA_CHATGPT_TOKEN_FILE", "CODEX_HOME"]);
        assert!(handed.iter().all(|(_, value)| !value.contains(token)));
        let file = PathBuf::from(codex_value(&handed, "NESSA_CHATGPT_TOKEN_FILE"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), token);
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                std::fs::metadata(file.parent().unwrap())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        let home = PathBuf::from(codex_value(&handed, "CODEX_HOME"));
        assert!(file.starts_with(&home));
        assert_eq!(home, root.path().join("codex-chatgpt"));
        let config: toml_edit::DocumentMut = std::fs::read_to_string(home.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(config["model_provider"].as_str(), Some("nessa_chatgpt"));
        assert_eq!(
            config["cli_auth_credentials_store"].as_str(),
            Some("ephemeral")
        );
        let provider = &config["model_providers"]["nessa_chatgpt"];
        assert!(provider.get("env_key").is_none());
        assert_eq!(provider["auth"]["command"].as_str(), Some("/bin/sh"));
        assert!(provider["auth"]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str().unwrap().contains("$NESSA_CHATGPT_TOKEN_FILE")));
        assert_eq!(
            provider["auth"]["refresh_interval_ms"].as_integer(),
            Some(300_000)
        );
        assert_eq!(
            provider["base_url"].as_str(),
            Some("https://api.openai.com/v1")
        );
        assert_eq!(provider["wire_api"].as_str(), Some("responses"));
        assert_eq!(provider["requires_openai_auth"].as_bool(), Some(false));
        assert_eq!(provider["supports_websockets"].as_bool(), Some(false));
        assert!(!config.to_string().contains(token));
        assert_eq!(
            codex_thread_rollout(&home, session_id)
                .map(|found| found.file_name().unwrap().to_owned()),
            Some(rollout.file_name().unwrap().to_owned())
        );
        assert!(!home.join("auth.json").exists());
    }
    // A renewal replaces the lease's token whole; a lease with no file
    // gets none, and an ended lease's file is gone.
    let renewed = "eyJhbGciOiJSUzI1NiJ9.eyJzdWIiOiJ5In0.bmV3";
    let renewed = Login::new(renewed.into()).unwrap();
    assert!(renew_chatgpt_token(&sessions, "lease-1", &renewed));
    let file = token_file(&root.path().join("codex-chatgpt"), "lease-1");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), renewed.expose());
    assert!(!renew_chatgpt_token(&sessions, "lease-2", &renewed));
    forget_chatgpt_token(&sessions, "lease-1");
    assert!(!file.exists());
    // As serving starts, a file a crash left behind is removed.
    let left =
        write_chatgpt_token(&root.path().join("codex-chatgpt"), "lease-3", &renewed).unwrap();
    let mut starting = ConfiguredLauncher::new("/srv/work".into(), HashMap::new());
    starting.codex_sessions(sessions.clone());
    starting.forget_chatgpt_tokens();
    assert!(!left.exists());
    // The key's home keeps OpenAI's own provider.
    let key_config = std::fs::read_to_string(key_home.join("config.toml")).unwrap();
    assert!(!key_config.contains("nessa_chatgpt"));
}

/// Something that is not this launcher's link where the shared sessions
/// go: the token is not handed rather than Codex losing its threads.
#[cfg(unix)]
#[test]
fn a_chatgpt_home_whose_sessions_are_not_the_shared_ones_is_not_used() {
    let login = Login::new("eyJh.eyJz.c2ln".into()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let sessions = root.path().join("codex");
    std::fs::create_dir_all(root.path().join("codex-chatgpt/sessions")).unwrap();
    assert_eq!(
        login_to_set(
            Some("lease-1"),
            AgentId::Codex,
            Some(&login),
            &BTreeMap::new(),
            |_| false,
            Some(&sessions)
        ),
        None
    );
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
        login_to_set(
            None,
            AgentId::Opencode,
            Some(&login),
            &own_key,
            |_| true,
            None
        ),
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
            None,
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
            login_to_set(None, AgentId::Codex, Some(&login), &BTreeMap::new(), |_| false, Some(&home))
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
            None,
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
