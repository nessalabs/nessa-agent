//! Exact-build lookup for the independent background helper, outside lease frames.
use base64::Engine;

/// Run the background helper from an exact installed build. Dynamic arguments
/// are base64 words inside the outer login-shell-safe script, as upload bytes are.
#[must_use]
pub(super) fn command(builds: &[&str], cwd: &str, timeout: u64, command: &str) -> String {
    // Only build digests form executable lookup syntax; arguments are data.
    assert!(
        !builds.is_empty()
            && builds
                .iter()
                .all(|build| !build.is_empty() && build.bytes().all(|b| b.is_ascii_hexdigit()))
    );
    let builds = builds.join(" ");
    let encode = |value: &str| base64::engine::general_purpose::STANDARD.encode(value);
    format!("sh -c 'c=$(printf %s {} | base64 --decode; printf .); c=${{c%?}}; w=$(printf %s {} | base64 --decode; printf .); w=${{w%?}}; for d in {}; do p=\"$HOME/{directory}/$d/nessa\"; if [ -x \"$p\" ]; then exec \"$p\" background-run --cwd \"$w\" --timeout {} -- bash --noprofile --norc -c \"$c\"; fi; done'", encode(command), encode(cwd), builds, timeout, directory = crate::env_serve::install::INSTALL_DIRECTORY)
}

#[cfg(test)]
#[path = "../../../../tests/conversation/background_command.rs"]
mod tests;
