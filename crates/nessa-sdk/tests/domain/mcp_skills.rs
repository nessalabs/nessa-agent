use nessa_sdk::domain::mcp_skills::{
    parse_frontmatter, FrontmatterValue, McpSkillError, SkillCatalog, SkillDefinition, SkillDigest,
    SkillEntry, SkillFile, SkillFileUri, SkillFrontmatter, SkillName, SkillResourceEntry,
    SkillResources, SkillUri, DIRECTORY_MIME_TYPE, MAX_FILES_PER_SKILL, MAX_SKILL_TOTAL_BYTES,
};
use std::collections::BTreeMap;

const SAMPLE_SKILL_MD: &str = r#"---
name: code-review
description: Review code using the team checklist.
license: Apache-2.0
metadata:
  version: "2.1.0"
  strict: true
allowed-tools:
  - read_file
  - grep
---

# Code review

Read `references/checklist.md` before reviewing.
"#;

const SAMPLE_CHECKLIST: &str = "# Checklist\nCheck tests and types.\n";

#[test]
fn skill_name_enforces_agent_skills_rules() {
    assert!(SkillName::new("code-review").is_ok());
    assert!(SkillName::new("a1").is_ok());
    assert!(SkillName::new("a".repeat(64)).is_ok());

    for bad in [
        "",
        "Code-Review",
        "-leading",
        "trailing-",
        "two--dashes",
        "has space",
        "has_underscore",
    ] {
        assert_eq!(
            SkillName::new(bad),
            Err(McpSkillError::InvalidSkillName),
            "{bad}"
        );
    }
    assert_eq!(
        SkillName::new("a".repeat(65)),
        Err(McpSkillError::InvalidSkillName)
    );
}

#[test]
fn skill_uri_extracts_name_and_blocks_traversal() {
    let simple = SkillUri::new("skill://code-review/SKILL.md").unwrap();
    assert_eq!(simple.skill_name().as_str(), "code-review");
    assert_eq!(simple.root_dir(), "skill://code-review");
    assert_eq!(
        simple
            .resolve_relative("references/checklist.md")
            .unwrap()
            .as_str(),
        "skill://code-review/references/checklist.md"
    );

    let nested = SkillUri::new("skill://acme/billing/refunds/SKILL.md").unwrap();
    assert_eq!(nested.skill_name().as_str(), "refunds");
    assert_eq!(nested.root_dir(), "skill://acme/billing/refunds");

    let custom_scheme = SkillUri::new("github://owner/repo/skills/refunds/SKILL.md").unwrap();
    assert_eq!(custom_scheme.skill_name().as_str(), "refunds");

    for bad_uri in [
        "skill://code-review/README.md",
        "skill://code-review/",
        "skill://../escape/SKILL.md",
        "skill://code-review/SKILL.md?x=1",
        "skill://code-review/SKILL.md#frag",
        "no-scheme/code-review/SKILL.md",
        "skill://BadName/SKILL.md",
    ] {
        assert_eq!(
            SkillUri::new(bad_uri),
            Err(McpSkillError::InvalidSkillUri),
            "{bad_uri}"
        );
    }

    for bad_rel in [
        "",
        "/abs.md",
        "../secret.md",
        "references/../../secret.md",
        "references/./a.md",
        "references\\a.md",
        "references/",
        "has space.md",
    ] {
        assert_eq!(
            simple.resolve_relative(bad_rel),
            Err(McpSkillError::InvalidRelativePath),
            "{bad_rel}"
        );
    }
}

#[test]
fn frontmatter_parser_preserves_nested_fields_and_rejects_malformed_input() {
    let fm = parse_frontmatter(SAMPLE_SKILL_MD).unwrap();
    assert_eq!(fm.name().as_str(), "code-review");
    assert_eq!(fm.description(), "Review code using the team checklist.");
    assert_eq!(
        fm.fields().get("license"),
        Some(&FrontmatterValue::String("Apache-2.0".to_owned()))
    );
    assert_eq!(
        fm.fields().get("metadata"),
        Some(&FrontmatterValue::Mapping(BTreeMap::from([
            ("strict".to_owned(), FrontmatterValue::Bool(true)),
            (
                "version".to_owned(),
                FrontmatterValue::String("2.1.0".to_owned())
            ),
        ])))
    );
    assert_eq!(
        fm.fields().get("allowed-tools"),
        Some(&FrontmatterValue::Sequence(vec![
            FrontmatterValue::String("read_file".to_owned()),
            FrontmatterValue::String("grep".to_owned()),
        ]))
    );

    assert!(matches!(
        parse_frontmatter("# No frontmatter"),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
    assert!(matches!(
        parse_frontmatter("---\nname: only-name\n---\n"),
        Err(McpSkillError::MissingFrontmatterField("description"))
    ));
    assert!(matches!(
        parse_frontmatter("---\nname: dup\nname: dup2\ndescription: ok\n---\n"),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
    assert!(matches!(
        parse_frontmatter("---\nname: bad\n\tdescription: tab\n---\n"),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
}

#[test]
fn skill_entry_enforces_manifest_invariants_and_sep2640_limits() {
    let uri = SkillUri::new("skill://code-review/SKILL.md").unwrap();
    let fm = parse_frontmatter(SAMPLE_SKILL_MD).unwrap();
    let skill_file_uri = SkillFileUri::from(&uri);
    let skill_digest = SkillDigest::from_bytes(SAMPLE_SKILL_MD.as_bytes());
    let skill_size = SAMPLE_SKILL_MD.len() as u64;

    // Name mismatch between URI and frontmatter.
    let other_uri = SkillUri::new("skill://other-name/SKILL.md").unwrap();
    assert!(matches!(
        SkillEntry::new(other_uri, fm.clone(), SkillResources::Dynamic),
        Err(McpSkillError::NameMismatch { .. })
    ));

    // Empty manifest rejected.
    assert_eq!(
        SkillEntry::new(uri.clone(), fm.clone(), SkillResources::Manifest(vec![])),
        Err(McpSkillError::EmptyManifest)
    );

    // Missing SKILL.md in manifest rejected.
    let ref_uri = uri.resolve_relative("references/checklist.md").unwrap();
    assert_eq!(
        SkillEntry::new(
            uri.clone(),
            fm.clone(),
            SkillResources::Manifest(vec![SkillResourceEntry::new(
                ref_uri.clone(),
                SkillDigest::from_bytes(SAMPLE_CHECKLIST.as_bytes()),
                SAMPLE_CHECKLIST.len() as u64,
            )]),
        ),
        Err(McpSkillError::MissingSkillMdInManifest)
    );

    // File outside skill root rejected.
    let outside = SkillFileUri::new("skill://other/secret.md").unwrap();
    assert!(matches!(
        SkillEntry::new(
            uri.clone(),
            fm.clone(),
            SkillResources::Manifest(vec![
                SkillResourceEntry::new(skill_file_uri.clone(), skill_digest.clone(), skill_size),
                SkillResourceEntry::new(outside, SkillDigest::from_bytes(b"x"), 1),
            ]),
        ),
        Err(McpSkillError::ManifestFileOutsideSkillRoot(_))
    ));

    // Exceeding 512 files rejected.
    let mut too_many = vec![SkillResourceEntry::new(
        skill_file_uri.clone(),
        skill_digest.clone(),
        skill_size,
    )];
    for i in 0..MAX_FILES_PER_SKILL {
        let extra = uri.resolve_relative(&format!("f/{i}.txt")).unwrap();
        too_many.push(SkillResourceEntry::new(
            extra,
            SkillDigest::from_bytes(b"x"),
            1,
        ));
    }
    assert_eq!(
        SkillEntry::new(uri.clone(), fm.clone(), SkillResources::Manifest(too_many)),
        Err(McpSkillError::TooManyFiles {
            count: MAX_FILES_PER_SKILL + 1,
            max: MAX_FILES_PER_SKILL,
        })
    );

    // Exceeding 16 MiB total size rejected.
    assert!(matches!(
        SkillEntry::new(
            uri.clone(),
            fm.clone(),
            SkillResources::Manifest(vec![SkillResourceEntry::new(
                skill_file_uri,
                skill_digest,
                MAX_SKILL_TOTAL_BYTES + 1
            ),]),
        ),
        Err(McpSkillError::TotalSizeExceeded { .. })
    ));

    // Dynamic skills are accepted with valid URI/frontmatter.
    assert!(SkillEntry::new(uri, fm, SkillResources::Dynamic).is_ok());
}

#[test]
fn verify_file_rejects_unlisted_size_digest_and_frontmatter_mismatches() {
    let catalog = SkillCatalog::new(vec![SkillDefinition {
        skill_path: "code-review".to_owned(),
        files: vec![
            SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD),
            SkillFile::markdown("references/checklist.md", SAMPLE_CHECKLIST),
        ],
    }])
    .unwrap();
    let entry = catalog.get("skill://code-review/SKILL.md").unwrap();
    let skill_uri = SkillFileUri::from(entry.uri());
    let ref_uri = entry
        .uri()
        .resolve_relative("references/checklist.md")
        .unwrap();

    // Valid files pass.
    assert!(entry
        .verify_file(&skill_uri, SAMPLE_SKILL_MD.as_bytes())
        .is_ok());
    assert!(entry
        .verify_file(&ref_uri, SAMPLE_CHECKLIST.as_bytes())
        .is_ok());

    // Unlisted URI rejected.
    let unlisted = entry.uri().resolve_relative("references/other.md").unwrap();
    assert!(matches!(
        entry.verify_file(&unlisted, b"hello"),
        Err(McpSkillError::UnlistedResource(_))
    ));

    // Size mismatch rejected.
    assert!(matches!(
        entry.verify_file(&ref_uri, b"short"),
        Err(McpSkillError::SizeMismatch { .. })
    ));

    // Same length, tampered bytes -> digest mismatch rejected.
    let mut tampered = SAMPLE_CHECKLIST.as_bytes().to_vec();
    tampered[0] = b'X';
    assert!(matches!(
        entry.verify_file(&ref_uri, &tampered),
        Err(McpSkillError::DigestMismatch { .. })
    ));

    // Dynamic skill still verifies SKILL.md frontmatter field-by-field.
    let dynamic_entry = SkillEntry::new(
        entry.uri().clone(),
        SkillFrontmatter::new(entry.frontmatter().fields().clone()).unwrap(),
        SkillResources::Dynamic,
    )
    .unwrap();
    let altered_fm = SAMPLE_SKILL_MD.replace("Apache-2.0", "MIT");
    assert_eq!(
        dynamic_entry.verify_file(&skill_uri, altered_fm.as_bytes()),
        Err(McpSkillError::FrontmatterMismatch)
    );
}

#[test]
fn skill_catalog_lists_directories_and_changes_digest_inputs_on_edit() {
    let catalog = SkillCatalog::new(vec![SkillDefinition {
        skill_path: "code-review".to_owned(),
        files: vec![
            SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD),
            SkillFile::markdown("references/checklist.md", SAMPLE_CHECKLIST),
        ],
    }])
    .unwrap();

    let root_children = catalog.read_directory("skill://code-review").unwrap();
    assert_eq!(root_children.len(), 2);
    assert!(root_children
        .iter()
        .any(|c| c.name == "references" && c.mime_type == DIRECTORY_MIME_TYPE));
    assert!(root_children
        .iter()
        .any(|c| c.name == "SKILL.md" && c.mime_type == "text/markdown"));

    let ref_children = catalog
        .read_directory("skill://code-review/references")
        .unwrap();
    assert_eq!(ref_children.len(), 1);
    assert_eq!(
        ref_children[0].uri,
        "skill://code-review/references/checklist.md"
    );

    // Trailing slash rejected.
    assert!(matches!(
        catalog.read_directory("skill://code-review/"),
        Err(McpSkillError::InvalidDirectoryUri(_))
    ));
    // Unknown directory rejected.
    assert!(matches!(
        catalog.read_directory("skill://code-review/missing"),
        Err(McpSkillError::DirectoryNotFound(_))
    ));

    // Changing any file changes digest_inputs().
    let updated = SkillCatalog::new(vec![SkillDefinition {
        skill_path: "code-review".to_owned(),
        files: vec![
            SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD),
            SkillFile::markdown("references/checklist.md", "# Updated\n"),
        ],
    }])
    .unwrap();
    assert_ne!(catalog.digest_inputs(), updated.digest_inputs());
    assert!(!catalog.is_empty());
    assert_eq!(catalog.entries().len(), 1);
    assert_eq!(catalog.list_resources().len(), 2);
    assert!(catalog
        .read_resource("skill://code-review/SKILL.md")
        .is_some());
    assert!(catalog
        .read_resource("skill://code-review/missing")
        .is_none());

    // Duplicate skill URI in catalog is rejected.
    let dup = SkillCatalog::new(vec![
        SkillDefinition {
            skill_path: "code-review".to_owned(),
            files: vec![SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD)],
        },
        SkillDefinition {
            skill_path: "code-review".to_owned(),
            files: vec![SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD)],
        },
    ]);
    assert!(matches!(dup, Err(McpSkillError::DuplicateSkillUri(_))));

    // Missing SKILL.md or invalid UTF-8 SKILL.md in catalog is rejected.
    assert_eq!(
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: "code-review".to_owned(),
            files: vec![SkillFile::markdown("references/a.md", "hi")],
        }])
        .unwrap_err(),
        McpSkillError::MissingSkillMdInManifest
    );
    assert!(matches!(
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: "code-review".to_owned(),
            files: vec![SkillFile {
                relative_path: "SKILL.md".to_owned(),
                mime_type: "text/markdown".to_owned(),
                bytes: vec![0xff, 0xfe],
            }],
        }]),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
}

#[test]
fn frontmatter_edge_cases_directory_children_and_error_display_are_covered() {
    // Comments, CRLF, BOM, quoted keys/values, escapes, inline lists, block scalars,
    // booleans, integers, null, same-indent sequences, and `...` terminator.
    let rich = concat!(
        "\u{feff}---\r\n",
        "# leading comment\r\n",
        "\"name\": 'rich-skill'\r\n",
        "'description': \"Line\\n\\r\\t\\/\\\"\\\\\\x\" # inline comment\r\n",
        "empty_val:\r\n",
        "null_val: null\r\n",
        "tilde_val: ~\r\n",
        "flag_false: false\r\n",
        "count: 42\r\n",
        "inline_empty: []\r\n",
        "inline_list: ['a''b', \"c,d\", 7]\r\n",
        "block_lit: |\r\n",
        "  first\r\n",
        "\r\n",
        "  second\r\n",
        "block_fold: >-\r\n",
        "  hello\r\n",
        "  world\r\n",
        "same_indent_seq:\r\n",
        "- one\r\n",
        "- \r\n",
        "  nested_key: ok\r\n",
        "...\r\n",
        "body\r\n",
    );
    let fm = parse_frontmatter(rich).unwrap();
    assert_eq!(fm.name().as_str(), "rich-skill");
    assert_eq!(
        fm.fields().get("count"),
        Some(&FrontmatterValue::Integer(42))
    );
    assert_eq!(
        fm.fields().get("flag_false"),
        Some(&FrontmatterValue::Bool(false))
    );
    assert_eq!(fm.fields().get("null_val"), Some(&FrontmatterValue::Null));
    assert_eq!(fm.fields().get("tilde_val"), Some(&FrontmatterValue::Null));
    assert_eq!(fm.fields().get("empty_val"), Some(&FrontmatterValue::Null));
    assert_eq!(
        fm.fields().get("inline_empty"),
        Some(&FrontmatterValue::Sequence(vec![]))
    );
    assert_eq!(
        fm.fields().get("block_fold"),
        Some(&FrontmatterValue::String("hello world".to_owned()))
    );

    // Frontmatter parser error paths.
    for bad in [
        "",
        "---\nname: a\ndescription: b\n",
        "---\n  indented: x\n---\n",
        "---\nnot_a_pair\n---\n",
        "---\n: empty-key\n---\n",
        "---\nname: a\ndescription: b\nitems:\n  - ok\n    bad: indent\n---\n",
        "---\nname: a\ndescription: b\nlist: [\"unterminated]\n---\n",
        "---\nname: a\ndescription: \"dangling\\\"\n---\n",
        "---\ndescription: no-name\n---\n",
    ] {
        assert!(parse_frontmatter(bad).is_err(), "{bad:?}");
    }

    // SkillDigest parse error paths and SkillFileUri error paths.
    assert_eq!(
        SkillDigest::parse("md5:abc"),
        Err(McpSkillError::InvalidDigest)
    );
    assert_eq!(
        SkillDigest::parse("sha256:ZZZZ"),
        Err(McpSkillError::InvalidDigest)
    );
    assert_eq!(
        SkillFileUri::new("skill://a/dir/"),
        Err(McpSkillError::InvalidFileUri)
    );
    assert_eq!(
        SkillFileUri::new("1bad://a/file.md"),
        Err(McpSkillError::InvalidFileUri)
    );

    // SkillEntry::directory_children (files of different extensions, nested dirs, errors).
    let catalog = SkillCatalog::new(vec![SkillDefinition {
        skill_path: "dir-test".to_owned(),
        files: vec![
            SkillFile::markdown(
                "SKILL.md",
                "---\nname: dir-test\ndescription: Directory test.\n---\n",
            ),
            SkillFile {
                relative_path: "data/a.json".to_owned(),
                mime_type: "application/json".to_owned(),
                bytes: b"{}".to_vec(),
            },
            SkillFile {
                relative_path: "data/b.yaml".to_owned(),
                mime_type: "application/yaml".to_owned(),
                bytes: b"k: v".to_vec(),
            },
            SkillFile {
                relative_path: "data/c.txt".to_owned(),
                mime_type: "text/plain".to_owned(),
                bytes: b"hi".to_vec(),
            },
            SkillFile {
                relative_path: "data/d.bin".to_owned(),
                mime_type: "application/octet-stream".to_owned(),
                bytes: vec![0, 1],
            },
        ],
    }])
    .unwrap();
    let entry = catalog.get("skill://dir-test/SKILL.md").unwrap();
    assert_eq!(
        entry.directory_children("skill://dir-test").unwrap().len(),
        2
    );
    assert_eq!(
        entry
            .directory_children("skill://dir-test/data")
            .unwrap()
            .len(),
        4
    );
    assert!(matches!(
        entry.directory_children("skill://dir-test/"),
        Err(McpSkillError::InvalidDirectoryUri(_))
    ));
    assert!(matches!(
        entry.directory_children("skill://dir-test/nope"),
        Err(McpSkillError::DirectoryNotFound(_))
    ));

    // Dynamic entry directory_children and outside-root verify_file rejection.
    let dyn_entry = SkillEntry::new(
        entry.uri().clone(),
        entry.frontmatter().clone(),
        SkillResources::Dynamic,
    )
    .unwrap();
    assert!(matches!(
        dyn_entry.directory_children("skill://dir-test"),
        Err(McpSkillError::DirectoryNotFound(_))
    ));
    assert!(matches!(
        dyn_entry.verify_file(&SkillFileUri::new("skill://other/x.md").unwrap(), b"x"),
        Err(McpSkillError::UnlistedResource(_))
    ));
    assert!(matches!(
        dyn_entry.verify_file(&SkillFileUri::from(entry.uri()), &[0xff, 0xfe]),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));

    // Duplicate manifest URI rejection.
    let skill_uri = entry.uri().clone();
    let file_uri = SkillFileUri::from(&skill_uri);
    let dig = SkillDigest::from_bytes(b"x");
    assert!(matches!(
        SkillEntry::new(
            skill_uri,
            entry.frontmatter().clone(),
            SkillResources::Manifest(vec![
                SkillResourceEntry::new(file_uri.clone(), dig.clone(), 1),
                SkillResourceEntry::new(file_uri, dig, 1),
            ]),
        ),
        Err(McpSkillError::DuplicateManifestUri(_))
    ));

    // Every McpSkillError Display arm formats non-empty text.
    let all_errors = [
        McpSkillError::InvalidSkillName,
        McpSkillError::InvalidSkillUri,
        McpSkillError::InvalidFileUri,
        McpSkillError::InvalidRelativePath,
        McpSkillError::InvalidDigest,
        McpSkillError::InvalidFrontmatter("bad".into()),
        McpSkillError::MissingFrontmatterField("name"),
        McpSkillError::NameMismatch {
            uri_name: "a".into(),
            frontmatter_name: "b".into(),
        },
        McpSkillError::EmptyManifest,
        McpSkillError::MissingSkillMdInManifest,
        McpSkillError::DuplicateManifestUri("u".into()),
        McpSkillError::ManifestFileOutsideSkillRoot("u".into()),
        McpSkillError::TooManyFiles {
            count: 513,
            max: 512,
        },
        McpSkillError::TotalSizeExceeded { bytes: 2, max: 1 },
        McpSkillError::UnlistedResource("u".into()),
        McpSkillError::SizeMismatch {
            expected: 1,
            actual: 2,
        },
        McpSkillError::DigestMismatch {
            expected: "a".into(),
            actual: "b".into(),
        },
        McpSkillError::FrontmatterMismatch,
        McpSkillError::InvalidDirectoryUri("u".into()),
        McpSkillError::DirectoryNotFound("u".into()),
        McpSkillError::DuplicateSkillUri("u".into()),
    ];
    for err in all_errors {
        assert!(!err.to_string().is_empty());
    }
}

#[test]
fn frontmatter_blocks_sequences_and_scalars_cover_every_layout() {
    let parse = |body: &str| {
        parse_frontmatter(&format!(
            "---\nname: code-review\ndescription: d\n{body}---\n"
        ))
    };
    let field = |body: &str, key: &str| parse(body).unwrap().fields()[key].clone();
    let invalid = |message: &str| Err(McpSkillError::InvalidFrontmatter(message.to_owned()));
    let string = |text: &str| FrontmatterValue::String(text.to_owned());

    let nested = BTreeMap::from([("b".to_owned(), FrontmatterValue::Integer(1))]);
    assert_eq!(
        field("a:\n\n  b: 1\n", "a"),
        FrontmatterValue::Mapping(nested)
    );
    assert_eq!(field("last:\n", "last"), FrontmatterValue::Null);
    assert_eq!(
        field("last:\n\n# trailing comment\n", "last"),
        FrontmatterValue::Null
    );
    assert_eq!(
        field("a:\n  - x\n\n  - y\n", "a"),
        FrontmatterValue::Sequence(vec![string("x"), string("y")])
    );
    let dedent = parse("a:\n  - x\nb: 1\n").unwrap();
    assert_eq!(
        dedent.fields()["a"],
        FrontmatterValue::Sequence(vec![string("x")])
    );
    assert_eq!(dedent.fields()["b"], FrontmatterValue::Integer(1));
    let flat = parse("a:\n- x\nb: 1\n").unwrap();
    assert_eq!(
        flat.fields()["a"],
        FrontmatterValue::Sequence(vec![string("x")])
    );
    assert_eq!(flat.fields()["b"], FrontmatterValue::Integer(1));
    assert_eq!(
        parse("a:\n  - x\n    - y\n"),
        invalid("unexpected sequence indentation at line 6")
    );
    assert_eq!(
        parse("a:\n  - x\n  b: 1\n"),
        invalid("unexpected indentation at line 6")
    );
    let tabs = "tabs are not permitted for YAML indentation";
    assert_eq!(parse("a:\n\tb: 1\n"), invalid(tabs));
    assert_eq!(parse("a:\n  - x\n\t- y\n"), invalid(tabs));

    assert_eq!(
        parse("a: |\n    x\n  y\nb: 1\n"),
        invalid("unexpected indentation at line 6")
    );
    let block = parse("a: |\n  x\n\n\nb: 1\n").unwrap();
    assert_eq!(block.fields()["a"], string("x\n"));
    assert_eq!(block.fields()["b"], FrontmatterValue::Integer(1));
    assert_eq!(field("a: >-\n  x\n  y\n", "a"), string("x y"));
    assert_eq!(field("a: >\n  x\n  y\n", "a"), string("x y\n"));
    assert_eq!(field("a: |-\n  x\n  y\n", "a"), string("x\ny"));

    assert_eq!(
        field("a: [\"q\\\"r\", 's']\n", "a"),
        FrontmatterValue::Sequence(vec![string("q\"r"), string("s")])
    );
    assert_eq!(field("a:b: c\n", "a:b"), string("c"));

    let unterminated = "unterminated quote in frontmatter value";
    assert_eq!(parse("\"a\\\": 1\n"), invalid(unterminated));
    assert_eq!(parse("a: \"x\\\"\n"), invalid(unterminated));
    assert_eq!(parse("a: [\"x]\n"), invalid(unterminated));
    assert_eq!(parse("a:\n  - \"x\\\"\n"), invalid(unterminated));
    assert_eq!(
        field("\"q\\tk\": 1\n", "q\tk"),
        FrontmatterValue::Integer(1)
    );
    assert_eq!(parse("a: |\n\tx\n"), invalid(tabs));
    assert_eq!(parse("a:\n  -\n\t  x: 1\n"), invalid(tabs));
    assert_eq!(
        parse("a:\n  b: 1\n  b: 2\n"),
        invalid("duplicate frontmatter key 'b'")
    );
    assert_eq!(
        parse_frontmatter("---\nname: Bad_Name\ndescription: d\n---\n"),
        Err(McpSkillError::InvalidSkillName)
    );
}

#[test]
fn skill_uris_catalog_paths_and_manifest_sizes_reject_edge_cases() {
    for bad in [
        String::new(),
        "skill:///SKILL.md".to_owned(),
        format!("skill://{}/SKILL.md", "a".repeat(5000)),
    ] {
        assert_eq!(SkillUri::new(bad), Err(McpSkillError::InvalidSkillUri));
    }
    let computed = SkillDigest::from_bytes(SAMPLE_CHECKLIST.as_bytes());
    assert_eq!(SkillDigest::parse(computed.as_str()), Ok(computed.clone()));
    assert_eq!(
        SkillDigest::parse(format!("sha256:{}", "0".repeat(63))),
        Err(McpSkillError::InvalidDigest)
    );

    let file = SkillFileUri::new("skill://code-review/SKILL.md").unwrap();
    assert_eq!(file.relative_to("skill:"), None);
    assert_eq!(file.relative_to("skill://code-rev"), None);
    assert!(SkillUri::new("my+skill.v-1://code-review/SKILL.md").is_ok());
    assert_eq!(
        SkillUri::new("my_skill://code-review/SKILL.md"),
        Err(McpSkillError::InvalidSkillUri)
    );

    let catalog = |skill_path: &str| {
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: skill_path.to_owned(),
            files: vec![SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD)],
        }])
        .map(|_| ())
    };
    assert_eq!(catalog("has space"), Err(McpSkillError::InvalidSkillUri));
    let catalog_of = |files: Vec<SkillFile>| {
        SkillCatalog::new(vec![SkillDefinition {
            skill_path: "code-review".to_owned(),
            files,
        }])
        .map(|_| ())
    };
    assert!(matches!(
        catalog_of(vec![SkillFile::markdown("SKILL.md", "no frontmatter")]),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
    assert_eq!(
        catalog_of(vec![
            SkillFile::markdown("SKILL.md", SAMPLE_SKILL_MD),
            SkillFile::markdown("../escape.md", SAMPLE_CHECKLIST),
        ]),
        Err(McpSkillError::InvalidRelativePath)
    );
    assert_eq!(
        catalog("other"),
        Err(McpSkillError::NameMismatch {
            uri_name: "other".to_owned(),
            frontmatter_name: "code-review".to_owned(),
        })
    );

    let uri = SkillUri::new("skill://code-review/SKILL.md").unwrap();
    let frontmatter = parse_frontmatter(SAMPLE_SKILL_MD).unwrap();
    let digest = SkillDigest::from_bytes(b"");
    let manifest = vec![
        SkillResourceEntry::new(SkillFileUri::from(&uri), digest.clone(), 1),
        SkillResourceEntry::new(
            uri.resolve_relative("references/big.md").unwrap(),
            digest,
            u64::MAX,
        ),
    ];
    let unparsable = b"no frontmatter";
    let listed = SkillEntry::new(
        uri.clone(),
        frontmatter.clone(),
        SkillResources::Manifest(vec![SkillResourceEntry::new(
            SkillFileUri::from(&uri),
            SkillDigest::from_bytes(unparsable),
            unparsable.len() as u64,
        )]),
    )
    .unwrap();
    assert!(matches!(
        listed.verify_file(&SkillFileUri::from(&uri), unparsable),
        Err(McpSkillError::InvalidFrontmatter(_))
    ));
    assert_eq!(
        SkillEntry::new(uri, frontmatter, SkillResources::Manifest(manifest)).map(|_| ()),
        Err(McpSkillError::TotalSizeExceeded {
            bytes: u64::MAX,
            max: MAX_SKILL_TOTAL_BYTES,
        })
    );
}
