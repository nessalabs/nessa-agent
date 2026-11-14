//! The stored list's edits as pure rules: which entry an edit names, the
//! reserved name, and a kept value.
use super::{
    ConfiguredMcpServer, EditRefusal, EnvironmentNameRepeated, RemoteConfigured, RemoteServerSave,
    ServerEdit, ServerSave, StdioServer, StoredMcpServer, MANAGED_SERVER_NAME,
};

fn server(name: &str) -> StdioServer {
    StdioServer::new(name, "/bin/server", vec![])
}

fn stored(name: &str, env: &[(&str, &str)]) -> StoredMcpServer {
    let env = env
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()));
    StoredMcpServer::Stdio(ConfiguredMcpServer::new(server(name), true, env).unwrap())
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

fn names(list: &[StoredMcpServer]) -> Vec<&str> {
    list.iter().map(StoredMcpServer::name).collect()
}

#[test]
fn a_remote_rename_keeps_its_id_and_a_new_remote_takes_the_id_it_was_given() {
    let kept = uuid::Uuid::from_u128(1);
    let list = vec![StoredMcpServer::Remote(RemoteConfigured::new(
        kept,
        "old",
        "https://mcp.example/a",
        true,
    ))];
    let renamed = ServerEdit::SaveRemote(RemoteServerSave {
        previous_name: Some("old".into()),
        id: uuid::Uuid::from_u128(2),
        name: "new".into(),
        url: "https://mcp.example/a".into(),
        enabled: true,
    })
    .apply(&list)
    .unwrap();
    assert_eq!(renamed[0].remote().unwrap().id(), kept);
    assert_eq!(renamed[0].name(), "new");
    let minted = uuid::Uuid::from_u128(3);
    let added = ServerEdit::SaveRemote(RemoteServerSave {
        previous_name: None,
        id: minted,
        name: "other".into(),
        url: "https://mcp.example/b".into(),
        enabled: false,
    })
    .apply(&list)
    .unwrap();
    assert_eq!(added[1].remote().unwrap().id(), minted);
    assert!(!added[1].enabled());
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
    assert_eq!(renamed[0].stdio().unwrap().env()["TOKEN"], "old");
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

/// A kept value stays with its launch: a save that changes anything the
/// process is started with, apart from the kept values themselves — the
/// command, the arguments, a variable added, another variable's value, a
/// variable left out — renamed or not, must give every value again.
/// Otherwise a new command, or a variable such as `LD_PRELOAD` that loads
/// code into the old one, could be pointed at a secret it was never given,
/// and read it back through an inspection (#480 adversarial review).
#[test]
fn a_kept_value_is_refused_when_anything_else_in_the_launch_changes() {
    let list = vec![stored("a", &[("TOKEN", "secret"), ("URL", "https://a")])];
    let relaunched =
        |previous: Option<&str>, command: &str, args: Vec<String>, env: &[(&str, Option<&str>)]| {
            ServerEdit::Save(ServerSave {
                previous_name: previous.map(str::to_owned),
                server: StdioServer::new("a", command, args),
                env: env
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), value.map(str::to_owned)))
                    .collect(),
                enabled: true,
            })
        };
    let same: &[(&str, Option<&str>)] = &[("TOKEN", None), ("URL", Some("https://a"))];
    for (why, edit) in [
        (
            "command",
            relaunched(None, "/usr/bin/python3", vec![], same),
        ),
        (
            "the command, as written",
            relaunched(None, "/bin//server", vec![], same),
        ),
        (
            "arguments",
            relaunched(None, "/bin/server", vec!["-c".into()], same),
        ),
        (
            "renamed, command",
            relaunched(Some("a"), "/usr/bin/python3", vec![], same),
        ),
        (
            "a variable added",
            relaunched(
                None,
                "/bin/server",
                vec![],
                &[
                    ("LD_PRELOAD", Some("/tmp/x.so")),
                    ("TOKEN", None),
                    ("URL", Some("https://a")),
                ],
            ),
        ),
        (
            "another variable's value",
            relaunched(
                None,
                "/bin/server",
                vec![],
                &[("TOKEN", None), ("URL", Some("https://evil"))],
            ),
        ),
        (
            "a variable swapped for another",
            relaunched(
                None,
                "/bin/server",
                vec![],
                &[("LD_PRELOAD", Some("/tmp/x.so")), ("TOKEN", None)],
            ),
        ),
        (
            "a variable left out",
            relaunched(None, "/bin/server", vec![], &[("TOKEN", None)]),
        ),
    ] {
        assert_eq!(
            edit.apply(&list),
            Err(EditRefusal::EnvironmentValueMissing {
                name: "TOKEN".into()
            }),
            "{why}"
        );
    }
    // The same launch keeps it, every other variable kept or given its
    // stored value, renamed or not.
    for (previous, env) in [
        (None, same),
        (None, &[("TOKEN", None), ("URL", None)][..]),
        (Some("a"), same),
    ] {
        let kept = relaunched(previous, "/bin/server", vec![], env)
            .apply(&list)
            .unwrap();
        assert_eq!(kept[0].stdio().unwrap().env()["TOKEN"], "secret");
        assert_eq!(kept[0].stdio().unwrap().env()["URL"], "https://a");
    }
    // With nothing kept, anything may change.
    let given = relaunched(
        None,
        "/usr/bin/python3",
        vec![],
        &[("LD_PRELOAD", Some("/x.so")), ("TOKEN", Some("new"))],
    )
    .apply(&list)
    .unwrap();
    assert_eq!(
        given[0].stdio().unwrap().env_names(),
        ["LD_PRELOAD", "TOKEN"]
    );
}
