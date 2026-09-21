use super::{SearchPath, SearchPathError};
#[cfg(target_os = "macos")]
use std::path::Path;

#[test]
fn the_system_path_is_the_floor_every_process_gets() {
    assert_eq!(
        SearchPath::system().as_str(),
        "/usr/bin:/bin:/usr/sbin:/sbin"
    );
}

#[test]
fn a_login_shell_path_keeps_its_absolute_entries_in_order() {
    let resolved = SearchPath::parse("/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin").unwrap();
    assert_eq!(
        resolved.as_str(),
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin"
    );
}

/// Empty and relative components are the two ways a `PATH` searches wherever a
/// tool happened to be run from. The agent runs in a workspace it writes to.
#[test]
fn empty_and_relative_entries_are_dropped_rather_than_searched() {
    for reported in [
        ":/usr/bin:/bin",
        "/usr/bin::/bin",
        "/usr/bin:/bin:",
        "/usr/bin:.:/bin",
        "/usr/bin:node_modules/.bin:/bin",
        "/usr/bin:../bin:/bin",
        "/usr/bin:~/bin:/bin",
    ] {
        assert_eq!(
            SearchPath::parse(reported).unwrap().as_str(),
            "/usr/bin:/bin",
            "{reported}"
        );
    }
}

#[test]
fn a_path_with_nothing_absolute_left_is_refused() {
    for reported in ["", ":", ".", "relative:./bin", "::"] {
        assert_eq!(
            SearchPath::parse(reported),
            Err(SearchPathError::NoAbsoluteEntry),
            "{reported}"
        );
    }
}

/// A shell that printed a banner, a prompt, an error or a NUL is not reporting
/// a `PATH`, and half of what it printed is not one either.
#[test]
fn control_characters_reject_the_whole_value() {
    for reported in [
        "/usr/bin\n/tmp/evil",
        "/usr/bin:/bin\n",
        "/usr/bin:\0/bin",
        "/usr/bin:/bin\r",
        "/usr/bin:\t/bin",
        "\u{7}/usr/bin",
    ] {
        assert_eq!(
            SearchPath::parse(reported),
            Err(SearchPathError::Control),
            "{reported:?}"
        );
    }
}

#[test]
fn an_unbounded_value_is_refused_before_it_is_parsed() {
    let longest = format!("/{}", "a".repeat(SearchPath::LIMIT - 1));
    assert_eq!(longest.len(), SearchPath::LIMIT);
    assert_eq!(SearchPath::parse(&longest).unwrap().as_str(), longest);
    assert_eq!(
        SearchPath::parse(&format!("{longest}a")),
        Err(SearchPathError::TooLong)
    );
}

/// The staged runtime holds Nessa's own `node`. Whatever the login shell says,
/// it does not become the `node` a project's tools find first.
///
/// Gated with what it exercises: only the host that stages a runtime has one to
/// exclude.
#[cfg(target_os = "macos")]
#[test]
fn the_runtime_directory_is_excluded_wherever_it_appears() {
    let runtime = Path::new("/Users/me/Library/Application Support/Nessa/gateway-runtimes/abc");
    let resolved = SearchPath::parse(&format!(
        "{}:/opt/homebrew/bin:/usr/bin:{}",
        runtime.display(),
        runtime.display()
    ))
    .unwrap();
    assert_eq!(
        resolved.excluding(runtime).unwrap().as_str(),
        "/opt/homebrew/bin:/usr/bin"
    );
    let untouched = SearchPath::parse("/opt/homebrew/bin:/usr/bin").unwrap();
    assert_eq!(untouched.excluding(runtime), Some(untouched.clone()));
    // Excluding produces a replacement value; the original is unchanged.
    assert_eq!(untouched.as_str(), "/opt/homebrew/bin:/usr/bin");
}

#[cfg(target_os = "macos")]
#[test]
fn excluding_the_only_entry_leaves_no_path_at_all() {
    let runtime = Path::new("/staged/runtime");
    assert_eq!(
        SearchPath::parse("/staged/runtime")
            .unwrap()
            .excluding(runtime),
        None
    );
}

#[cfg(target_os = "macos")]
#[test]
fn a_directory_that_is_not_utf8_removes_nothing() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let resolved = SearchPath::parse("/opt/homebrew/bin:/usr/bin").unwrap();
    let invalid = OsString::from_vec(vec![b'/', 0xff, 0xfe]);
    assert_eq!(
        resolved.excluding(Path::new(&invalid)),
        Some(resolved.clone())
    );
}

#[test]
fn every_rejection_says_which_rule_it_broke() {
    for (error, wording) in [
        (SearchPathError::TooLong, "length limit"),
        (SearchPathError::Control, "control characters"),
        (SearchPathError::NoAbsoluteEntry, "no absolute entry"),
    ] {
        assert!(error.to_string().contains(wording), "{error:?}");
    }
}
