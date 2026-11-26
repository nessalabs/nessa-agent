//! Which programs come back after a restart: the person's switch and rules
//! (Settings › Terminal). Off until the person turns it on, and then only a
//! program a rule allows comes back.
//!
//! A rule names a program, and may narrow it to a folder and to arguments
//! the command must hold. **Never** rules win over **Allow** rules, and a
//! program no rule allows does not come back, so turning resume on resumes
//! nothing until the person names what may.
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The most rules a person may keep.
pub const MAX_RULES: usize = 64;
/// The most arguments one rule may require.
pub const MAX_RULE_ARGS: usize = 16;
/// The longest program, folder or argument text a rule may hold, in bytes.
pub const MAX_RULE_TEXT: usize = 1024;

/// Whether matching programs come back or never do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleAction {
    Allow,
    Never,
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeRule {
    /// `*` for every program; a name (`claude`, `nvim`) for a program by
    /// the last part of its path; an absolute path for exactly that file.
    pub program: String,
    /// Only where the program works in this folder or one inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Only when every one of these is an argument of the command, exactly.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    pub action: RuleAction,
}

/// The person's switch and rules.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumeSettings {
    /// Off: terminals are given no token, so no program can report, and no
    /// saved program comes back.
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<ResumeRule>,
}

/// Why settings were refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuleError {
    TooMany,
    Invalid(&'static str),
}

impl ResumeSettings {
    /// The settings as kept: each rule trimmed, and refused when any rule
    /// names nothing, or a folder that is not absolute, or is over a limit.
    pub fn checked(mut self) -> Result<Self, RuleError> {
        if self.rules.len() > MAX_RULES {
            return Err(RuleError::TooMany);
        }
        for rule in &mut self.rules {
            rule.program = rule.program.trim().to_owned();
            if rule.program.is_empty() || !fits(&rule.program) {
                return Err(RuleError::Invalid("program"));
            }
            if rule.program.contains('/') && !rule.program.starts_with('/') {
                return Err(RuleError::Invalid("program"));
            }
            if let Some(folder) = &mut rule.folder {
                *folder = folder.trim().trim_end_matches('/').to_owned();
                if folder.is_empty() {
                    *folder = "/".into();
                }
                if !folder.starts_with('/') || !fits(folder) {
                    return Err(RuleError::Invalid("folder"));
                }
            }
            if rule.args.len() > MAX_RULE_ARGS || rule.args.iter().any(|arg| !fits(arg)) {
                return Err(RuleError::Invalid("args"));
            }
        }
        Ok(self)
    }

    /// Whether `argv`, working in `folder`, comes back.
    pub fn allows(&self, argv: &[String], folder: Option<&str>) -> bool {
        if !self.enabled {
            return false;
        }
        let matching = || self.rules.iter().filter(|rule| rule.matches(argv, folder));
        !matching().any(|rule| rule.action == RuleAction::Never)
            && matching().any(|rule| rule.action == RuleAction::Allow)
    }
}

impl ResumeRule {
    fn matches(&self, argv: &[String], folder: Option<&str>) -> bool {
        let Some(program) = argv.first() else {
            return false;
        };
        let named = if self.program == "*" {
            true
        } else if self.program.starts_with('/') {
            program == &self.program
        } else {
            Path::new(program)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == self.program)
        };
        let placed = match (&self.folder, folder) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(rule), Some(folder)) => Path::new(folder).starts_with(Path::new(rule)),
        };
        let argued = self
            .args
            .iter()
            .all(|wanted| argv[1..].iter().any(|arg| arg == wanted));
        named && placed && argued
    }
}

fn fits(text: &str) -> bool {
    text.len() <= MAX_RULE_TEXT && !text.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(program: &str, action: RuleAction) -> ResumeRule {
        ResumeRule {
            program: program.into(),
            folder: None,
            args: Vec::new(),
            action,
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_owned()).collect()
    }

    #[test]
    fn nothing_comes_back_until_a_rule_allows_it() {
        let mut settings = ResumeSettings {
            enabled: false,
            rules: vec![rule("*", RuleAction::Allow)],
        };
        assert!(
            !settings.allows(&argv(&["nvim"]), None),
            "off resumes nothing"
        );
        settings.enabled = true;
        assert!(settings.allows(&argv(&["nvim"]), None));
        settings.rules.clear();
        assert!(
            !settings.allows(&argv(&["nvim"]), None),
            "no rule allows it"
        );
    }

    #[test]
    fn matches_by_name_path_folder_and_arguments() {
        let settings = ResumeSettings {
            enabled: true,
            rules: vec![
                rule("claude", RuleAction::Allow),
                rule("/opt/bin/nvim", RuleAction::Allow),
                ResumeRule {
                    program: "lazygit".into(),
                    folder: Some("/work".into()),
                    args: Vec::new(),
                    action: RuleAction::Allow,
                },
                ResumeRule {
                    program: "codex".into(),
                    folder: None,
                    args: vec!["resume".into()],
                    action: RuleAction::Allow,
                },
            ],
        };
        assert!(settings.allows(&argv(&["claude", "--resume", "x"]), None));
        assert!(settings.allows(&argv(&["/usr/local/bin/claude"]), None));
        assert!(settings.allows(&argv(&["/opt/bin/nvim"]), None));
        assert!(
            !settings.allows(&argv(&["nvim"]), None),
            "a path rule wants that path"
        );
        assert!(settings.allows(&argv(&["lazygit"]), Some("/work/app")));
        assert!(settings.allows(&argv(&["lazygit"]), Some("/work")));
        assert!(!settings.allows(&argv(&["lazygit"]), Some("/workshop")));
        assert!(!settings.allows(&argv(&["lazygit"]), None));
        assert!(settings.allows(&argv(&["codex", "resume", "id"]), None));
        assert!(!settings.allows(&argv(&["codex"]), None));
        assert!(!settings.allows(&argv(&["resume", "codex"]), None));
    }

    #[test]
    fn never_wins_over_allow() {
        let settings = ResumeSettings {
            enabled: true,
            rules: vec![
                rule("*", RuleAction::Allow),
                ResumeRule {
                    program: "*".into(),
                    folder: None,
                    args: vec!["--dangerously-skip-permissions".into()],
                    action: RuleAction::Never,
                },
                rule("ssh", RuleAction::Never),
            ],
        };
        assert!(settings.allows(&argv(&["claude"]), None));
        assert!(!settings.allows(&argv(&["claude", "--dangerously-skip-permissions"]), None));
        assert!(!settings.allows(&argv(&["/usr/bin/ssh", "box"]), None));
    }

    #[test]
    fn keeps_rules_trimmed_and_refuses_what_names_nothing() {
        let kept = ResumeSettings {
            enabled: true,
            rules: vec![ResumeRule {
                program: " claude ".into(),
                folder: Some("/work/".into()),
                args: Vec::new(),
                action: RuleAction::Allow,
            }],
        }
        .checked()
        .unwrap();
        assert_eq!(kept.rules[0].program, "claude");
        assert_eq!(kept.rules[0].folder.as_deref(), Some("/work"));
        for bad in ["", "bin/claude", "cla\nude"] {
            let settings = ResumeSettings {
                enabled: true,
                rules: vec![rule(bad, RuleAction::Allow)],
            };
            assert!(settings.checked().is_err(), "{bad:?}");
        }
        let relative = ResumeSettings {
            enabled: true,
            rules: vec![ResumeRule {
                program: "x".into(),
                folder: Some("work".into()),
                args: Vec::new(),
                action: RuleAction::Allow,
            }],
        };
        assert_eq!(relative.checked(), Err(RuleError::Invalid("folder")));
        let many = ResumeSettings {
            enabled: true,
            rules: vec![rule("x", RuleAction::Allow); MAX_RULES + 1],
        };
        assert_eq!(many.checked(), Err(RuleError::TooMany));
    }
}
