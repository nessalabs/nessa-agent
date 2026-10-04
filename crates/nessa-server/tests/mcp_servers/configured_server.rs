//! The stored list's edits as pure rules: which entry an edit names, the
//! reserved name, and a kept value.
use super::{
    ConfiguredMcpServer, EditRefusal, ServerEdit, ServerSave, StdioServer, MANAGED_SERVER_NAME,
};
use std::collections::BTreeMap;

fn server(name: &str) -> StdioServer {
    StdioServer {
        name: name.into(),
        command: "/bin/server".into(),
        args: vec![],
    }
}

fn stored(name: &str, env: &[(&str, &str)]) -> ConfiguredMcpServer {
    ConfiguredMcpServer {
        server: server(name),
        enabled: true,
        env: env
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
    }
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
    list.iter().map(|each| each.server.name.as_str()).collect()
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
    assert_eq!(renamed[0].env["TOKEN"], "old");
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
    let configured = ConfiguredMcpServer {
        env: BTreeMap::from([("API_TOKEN".to_owned(), "secret-value".to_owned())]),
        ..stored("a", &[])
    };
    let save = save("a", None, &[("API_TOKEN", Some("secret-value"))]);
    for printed in [format!("{configured:?}"), format!("{save:?}")] {
        assert!(printed.contains("API_TOKEN"), "{printed}");
        assert!(!printed.contains("secret-value"), "{printed}");
    }
}
