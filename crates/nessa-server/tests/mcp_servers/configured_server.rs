//! The stored list's edits as pure rules: which entry an edit names, the
//! reserved name, and a kept value.
use super::{
    ConfiguredMcpServer, EditRefusal, EnvironmentNameRepeated, ServerEdit, ServerSave, StdioServer,
    MANAGED_SERVER_NAME,
};

fn server(name: &str) -> StdioServer {
    StdioServer::new(name, "/bin/server", vec![])
}

fn stored(name: &str, env: &[(&str, &str)]) -> ConfiguredMcpServer {
    let env = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()));
    ConfiguredMcpServer::new(server(name), true, env).unwrap()
}

fn save(name: &str, previous: Option<&str>, env: &[(&str, Option<&str>)]) -> ServerEdit {
    ServerEdit::Save(ServerSave {
        previous_name: previous.map(str::to_owned),
        server: server(name),
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.map(str::to_owned)))
            .collect(),
        enabled: true,
    })
}

fn names(list: &[ConfiguredMcpServer]) -> Vec<&str> {
    list.iter().map(|each| each.server().name()).collect()
}

#[test]
fn a_save_adds_replaces_in_place_or_renames() {
    let list = vec![stored("a", &[]), stored("b", &[])];
    assert_eq!(
        names(&save("c", None, &[]).apply(&list).unwrap()),
        ["a", "b", "c"]
    );
    assert_eq!(
        names(&save("a", None, &[]).apply(&list).unwrap()),
        ["a", "b"]
    );
    assert_eq!(
        names(&save("z", Some("a"), &[]).apply(&list).unwrap()),
        ["z", "b"]
    );
    assert_eq!(
        save("z", Some("missing"), &[]).apply(&list),
        Err(EditRefusal::NotFound)
    );
}

#[test]
fn a_kept_value_comes_from_the_entry_being_replaced() {
    let list = vec![stored("a", &[("TOKEN", "old")])];
    let renamed = save("b", Some("a"), &[("TOKEN", None)])
        .apply(&list)
        .unwrap();
    assert_eq!(renamed[0].env()["TOKEN"], "old");
    assert_eq!(
        save("c", None, &[("TOKEN", None)]).apply(&list),
        Err(EditRefusal::EnvironmentValueMissing {
            name: "TOKEN".into()
        })
    );
    assert_eq!(
        save("a", None, &[("TOKEN", Some("1")), ("TOKEN", None)]).apply(&list),
        Err(EditRefusal::EnvironmentNameRepeated {
            name: "TOKEN".into()
        })
    );
}

#[test]
fn no_edit_names_the_managed_server() {
    let list = vec![stored("a", &[])];
    for edit in [
        save(MANAGED_SERVER_NAME, None, &[]),
        save("a", Some(MANAGED_SERVER_NAME), &[]),
        ServerEdit::Remove {
            name: MANAGED_SERVER_NAME.into(),
        },
    ] {
        assert_eq!(
            edit.apply(&list),
            Err(EditRefusal::ReservedName),
            "{edit:?}"
        );
    }
}

#[test]
fn a_remove_takes_out_only_its_name() {
    let list = vec![stored("a", &[]), stored("b", &[])];
    let remove = |name: &str| ServerEdit::Remove { name: name.into() };
    assert_eq!(names(&remove("a").apply(&list).unwrap()), ["b"]);
    assert_eq!(remove("c").apply(&list), Err(EditRefusal::NotFound));
}

#[test]
fn a_configured_server_prints_its_variable_names_never_their_values() {
    let configured = stored("a", &[("API_TOKEN", "secret-value")]);
    let save = save("a", None, &[("API_TOKEN", Some("secret-value"))]);
    for printed in [format!("{configured:?}"), format!("{save:?}")] {
        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(!printed.contains("secret-value"), "{printed}");
    }
}

/// A server's variables are named once: given twice — whatever the values,
/// in either order — the server is refused, naming the variable, rather than
/// keeping one value silently.
#[test]
fn a_configured_server_refuses_a_repeated_variable_name() {
    for env in [
        [("TOKEN", "first"), ("TOKEN", "second")],
        [("TOKEN", "second"), ("TOKEN", "first")],
        [("TOKEN", "first"), ("TOKEN", "first")],
    ] {
        let env = env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()));
        assert_eq!(
            ConfiguredMcpServer::new(server("a"), true, env),
            Err(EnvironmentNameRepeated {
                name: "TOKEN".into()
            })
        );
    }
    let kept = ConfiguredMcpServer::new(
        server("a"),
        true,
        [
            ("B".to_owned(), "2".to_owned()),
            ("A".to_owned(), "1".to_owned()),
        ],
    )
    .unwrap();
    assert_eq!(kept.env_names(), ["A", "B"]);
}

/// A variable given twice is said before a value missing for it: both are
/// wrong here, and the repetition is the one reported.
#[test]
fn a_repeated_name_is_said_before_a_missing_value() {
    let list = vec![stored("a", &[])];
    assert_eq!(
        save("a", None, &[("TOKEN", None), ("TOKEN", None)]).apply(&list),
        Err(EditRefusal::EnvironmentNameRepeated {
            name: "TOKEN".into()
        })
    );
}
