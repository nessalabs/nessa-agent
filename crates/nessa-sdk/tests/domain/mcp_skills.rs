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
}
