//! Process launch arguments, decided before a windowing toolkit starts.
//!
//! The desktop application has no command-line surface. Refusing arguments at
//! this boundary keeps a test-harness filter or flag from opening a real panel
//! when somebody accidentally selects the application binary under `target/`.

use std::ffi::{OsStr, OsString};

/// An argument supplied to an application that accepts none.
#[derive(Debug, PartialEq, Eq)]
pub struct UnexpectedArgument(OsString);

impl std::fmt::Display for UnexpectedArgument {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            output,
            "the Nessa desktop app does not accept command-line arguments (unexpected {})",
            shown(&self.0)
        )
    }
}

/// Accept an empty argument list and refuse the first supplied value.
///
/// The executable name is not part of `arguments`; callers pass only the
/// values after it. Refusal happens before Tauri or a platform window host is
/// initialized.
pub fn accept(arguments: impl IntoIterator<Item = OsString>) -> Result<(), UnexpectedArgument> {
    match arguments.into_iter().next() {
        Some(argument) => Err(UnexpectedArgument(argument)),
        None => Ok(()),
    }
}

fn shown(argument: &OsStr) -> String {
    format!("{:?}", argument.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_application_launch_accepts_no_arguments() {
        assert_eq!(accept(Vec::new()), Ok(()));
    }

    #[test]
    fn a_test_filter_is_refused_before_the_application_can_start() {
        let error = accept([
            OsString::from("login_shell"),
            OsString::from("--test-threads=1"),
        ])
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "the Nessa desktop app does not accept command-line arguments (unexpected \"login_shell\")"
        );
    }

    #[test]
    fn a_test_harness_flag_is_refused_too() {
        let error = accept([OsString::from("--test-threads=1")]).unwrap_err();

        assert!(error.to_string().contains("--test-threads=1"));
    }
}
