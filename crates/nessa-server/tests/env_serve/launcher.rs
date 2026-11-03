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
        login_to_set(AgentId::Claude, Some(&login), &none),
        Some(("CLAUDE_CODE_OAUTH_TOKEN", "sk-ant-oat-x"))
    );
    assert_eq!(
        login_to_set(AgentId::Codex, Some(&login), &none),
        Some(("CODEX_API_KEY", "sk-ant-oat-x"))
    );
    assert_eq!(login_to_set(AgentId::Claude, None, &none), None);
    // An API key the host set is the host's login too.
    let own = BTreeMap::from([(OsString::from("ANTHROPIC_API_KEY"), OsString::from("mine"))]);
    assert_eq!(login_to_set(AgentId::Claude, Some(&login), &own), None);
    // Another agent's variable is not this agent's login.
    assert_eq!(
        login_to_set(AgentId::Codex, Some(&login), &own),
        Some(("CODEX_API_KEY", "sk-ant-oat-x"))
    );
}
