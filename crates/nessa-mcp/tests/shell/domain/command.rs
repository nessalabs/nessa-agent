use super::*;
use crate::shell::domain::command::CommandValidationError;
#[test]
fn commands_preserve_exact_text_and_reject_invalid_execution_bounds() {
    let text = "  printf '%s' 'a; b'\n";
    assert_eq!(ShellCommand::new(text.into(), 120).unwrap().text(), text);
    assert_eq!(
        ShellCommand::new("".into(), 1),
        Err(CommandValidationError::Empty)
    );
    assert_eq!(
        ShellCommand::new(" \n".into(), 1),
        Err(CommandValidationError::Empty)
    );
    assert_eq!(
        ShellCommand::new("a\0b".into(), 1),
        Err(CommandValidationError::ContainsNul)
    );
    assert_eq!(
        ShellCommand::new("x".repeat(32769), 1),
        Err(CommandValidationError::TooLong { max_bytes: 32768 })
    );
    for seconds in [0, 3601] {
        assert_eq!(
            ShellCommand::new("true".into(), seconds),
            Err(CommandValidationError::TimeoutOutOfRange { seconds })
        );
    }
}
