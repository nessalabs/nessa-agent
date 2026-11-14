/// The lease protocol names every source of the lease contract: each that
/// writes or reads its frames, each of `env serve` (how the host launches,
/// records and cleans up), each binding declaring the variables a launch may
/// set, and the harness cleanup both ends report. A change to any of them
/// changes the protocol, so mismatched builds are refused at the hello.
#[test]
fn every_source_of_the_lease_contract_names_the_protocol() {
    fn sources(directory: &std::path::Path, all: bool, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, all, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if all
                    || text.contains("ToEnvironment")
                    || text.contains("FromEnvironment")
                    || text.contains("const LAUNCH_VARIABLES")
                {
                    found.push(path.canonicalize().unwrap());
                }
            }
        }
    }
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = crate_root.join("src");
    let named: Vec<_> = super::LEASE_PROTOCOL_SOURCES
        .iter()
        .map(|source| config.join(source).canonicalize().unwrap())
        .collect();
    let mut found = Vec::new();
    sources(&crate_root.join("src"), false, &mut found);
    sources(&crate_root.join("../nessa-server/src"), false, &mut found);
    sources(&crate_root.join("../nessa-env-serve/src"), true, &mut found);
    sources(&crate_root.join("../nessa-protocol/src"), false, &mut found);
    sources(&crate_root.join("../nessa-sdk/src"), false, &mut found);
    let sdk = crate_root.join("../nessa-sdk/src/infrastructure");
    for cleanup in ["harness_process.rs", "process.rs"] {
        found.push(sdk.join(cleanup).canonicalize().unwrap());
    }
    let launch_variables = found
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .contains("const LAUNCH_VARIABLES")
        })
        .count();
    assert!(launch_variables >= 2, "{found:?}");
    assert!(found.len() >= 12, "{found:?}");
    for source in found {
        assert!(
            named.contains(&source),
            "{source:?} is not in the lease protocol"
        );
    }
}
