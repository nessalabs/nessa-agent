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
        login_to_set(AgentId::Claude, Some(&login), &none, |_| false),
        Some(("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-x"))
    );
    assert_eq!(
        login_to_set(AgentId::Codex, Some(&login), &none, |_| false),
        Some(("CODEX_API_KEY", "sk-ant-oat-x"))
    );
    assert_eq!(login_to_set(AgentId::Claude, None, &none, |_| false), None);
    // A saved Anthropic API key goes where Claude Code reads one.
    let key = Login::new("sk-ant-api03-x".into()).unwrap();
    assert_eq!(
        login_to_set(AgentId::Claude, Some(&key), &none, |_| false),
        Some(("ANTHROPIC_API_KEY", "sk-ant-api03-x"))
    );
    // An API key the host set is the host's login too.
    let own = BTreeMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from("mine"))]);
    assert_eq!(
        login_to_set(AgentId::Claude, Some(&login), &own, |_| false),
        None
    );
    // Another agent's variable is not this agent's login.
    assert_eq!(
        login_to_set(AgentId::Codex, Some(&login), &own, |_| false),
        Some(("CODEX_API_KEY", "sk-ant-oat-x"))
    );
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
        login_to_set(AgentId::Opencode, Some(&login), &home, |_| true),
        Some(("OPENCODE_API_KEY", "sk-ant-oat-x"))
    );
    let own_key = BTreeMap::from([(OsString::from("OPENCODE_API_KEY"), OsString::from("mine"))]);
    assert_eq!(
        login_to_set(AgentId::Opencode, Some(&login), &own_key, |_| true),
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
    assert_eq!(
        login_to_set(AgentId::Codex, Some(&login), &pointed, signed_in),
        None
    );
    // Claude's file is somewhere else: not signed in, so it is set.
    assert_eq!(
        login_to_set(AgentId::Claude, Some(&login), &pointed, signed_in),
        Some(("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-x"))
    );
    // No HOME and nothing pointed: nowhere to look.
    assert!(own_sign_in_files(AgentId::Claude, &BTreeMap::new()).is_empty());
}
