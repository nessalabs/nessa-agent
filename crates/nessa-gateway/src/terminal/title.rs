//! Terminal titles: one a person gives, and the one a Run gets.

/// A name the person gave, as kept: trimmed, 1 to 80 characters, no
/// control characters. None when it is none of those.
pub fn named(title: &str) -> Option<String> {
    let title = title.trim();
    let count = title.chars().count();
    ((1..=80).contains(&count) && !title.chars().any(char::is_control)).then(|| title.to_owned())
}

/// The title of a Run: its first non-blank line, cut to 80 characters.
pub(super) fn run_title(command: &str) -> String {
    let line = command
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("run");
    let mut title: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        title.push('…');
    }
    title
}
