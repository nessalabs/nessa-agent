//! The hub's side of keeping programs across restarts (OSC 7577): the
//! token a new terminal is given, the records its engine reports, the
//! person's settings and kept programs, and the start that opens kept
//! terminals again.
//!
//! ```text
//! program ── OSC 7577 (mac with the token) ──▶ engine: ResumeTracker ── Saved/Forget ──▶ hub ──▶ store (0600 files)
//! gateway start ── resume_saved ──▶ store ── allowed by the rules? ──▶ open(reopen: same id, title) ──▶ shell runs env -- argv
//! gateway stop  ── freeze ──▶ nothing kept is forgotten while every terminal ends
//! ```
use super::*;

/// Why a kept terminal is not opened again.
#[derive(Debug)]
enum Skip {
    Off,
    OtherAccount,
    StartedOnce,
    NotAllowed,
    FolderGone,
    Unstartable,
}

impl TerminalHub {
    /// A token for a terminal about to start, when resume is on. None as
    /// root: a program run as root would be brought back as root.
    pub(super) fn resume_token(&self) -> Option<String> {
        let resume = self.resume.as_ref()?;
        (resume.settings().enabled && resume.processes.own_uid() != 0).then(new_token)
    }

    /// The record kept for `id` as it would be written now.
    pub(super) fn saved_terminal(&self, id: &str) -> Option<SavedTerminal> {
        let hub = self.lock();
        let live = hub.terminals.iter().find(|live| live.entry.id == id)?;
        let record = live.saved.clone()?;
        Some(SavedTerminal {
            id: live.entry.id.clone(),
            request_id: live.entry.request_id.clone(),
            title: live.entry.title.clone(),
            organization: live.owner.organization.clone(),
            principal: live.owner.principal.clone(),
            channel_id: live.entry.channel_id.clone(),
            conversation_id: live.entry.conversation_id.clone(),
            folder: live.entry.folder.clone(),
            cols: live.size.cols,
            rows: live.size.rows,
            record,
        })
    }

    /// The engine took or dropped `terminal`'s record.
    pub(super) fn resume_changed(&self, terminal: &str, change: ResumeChange) {
        let Some(resume) = &self.resume else {
            return;
        };
        if resume.frozen.load(Ordering::Acquire) {
            return;
        }
        let _writing = resume.writing();
        let now = self.clock.now_ms();
        let command = {
            let mut hub = self.lock();
            let Some(live) = hub
                .terminals
                .iter_mut()
                .find(|live| live.entry.id == terminal)
            else {
                return;
            };
            let command = match &change {
                ResumeChange::Saved(record) => {
                    if !resume.settings().enabled {
                        return;
                    }
                    live.saved = Some(SavedRecord {
                        pid: record.pid,
                        started: record.started,
                        uid: record.uid,
                        argv: record.argv.clone(),
                        cwd: record.cwd.clone(),
                        owner_cwd: record.owner_cwd.clone(),
                        time: record.time,
                        saved_at_ms: now,
                        resumed: false,
                    });
                    Some(display_command(&record.argv))
                }
                ResumeChange::Forget => {
                    live.saved = None;
                    None
                }
            };
            if live.entry.resume_command != command {
                live.entry.resume_command = command.clone();
                let owner = live.owner.clone();
                self.announce(&mut hub, &owner);
            }
            command
        };
        match command {
            Some(_) => {
                if let Some(saved) = self.saved_terminal(terminal) {
                    match resume.store.save(&saved) {
                        Ok(()) => tracing::info!(
                            target: "nessa::terminal",
                            terminal = %terminal,
                            program = %saved.record.argv.first().map(String::as_str).unwrap_or(""),
                            "terminal program will be kept across restarts"
                        ),
                        Err(error) => tracing::warn!(
                            target: "nessa::terminal",
                            terminal = %terminal,
                            %error,
                            "resume record not kept"
                        ),
                    }
                }
            }
            None => resume.forget(terminal),
        }
    }

    /// The gateway is stopping: every terminal is about to end with it, and
    /// none of that is the person ending a program, so nothing kept goes.
    pub fn freeze(&self) {
        if let Some(resume) = &self.resume {
            let _writing = resume.writing();
            resume.frozen.store(true, Ordering::Release);
        }
    }

    /// The person's resume settings; `None` on a gateway that keeps none.
    pub fn resume_settings(&self) -> Option<ResumeSettings> {
        self.resume.as_ref().map(Resume::settings)
    }

    /// Keep `settings` and answer what is kept. Turning resume off forgets
    /// every kept program; terminals already open keep their token, but
    /// nothing they report is kept.
    pub fn set_resume_settings(
        &self,
        settings: ResumeSettings,
    ) -> Result<ResumeSettings, ResumeSettingsError> {
        let resume = self
            .resume
            .as_ref()
            .ok_or(ResumeSettingsError::Unavailable)?;
        let settings = settings.checked().map_err(ResumeSettingsError::Invalid)?;
        let _writing = resume.writing();
        resume
            .store
            .save_settings(&settings)
            .map_err(ResumeSettingsError::Storage)?;
        if let Ok(mut held) = resume.settings.lock() {
            *held = settings.clone();
        }
        tracing::info!(
            target: "nessa::terminal",
            enabled = settings.enabled,
            rules = settings.rules.len(),
            "terminal resume settings changed"
        );
        if !settings.enabled {
            for saved in resume.store.saved() {
                resume.forget(&saved.id);
            }
            let mut hub = self.lock();
            let mut owners = Vec::new();
            for live in &mut hub.terminals {
                live.saved = None;
                if live.entry.resume_command.take().is_some() {
                    owners.push(live.owner.clone());
                }
            }
            owners.dedup();
            for owner in owners {
                self.announce(&mut hub, &owner);
            }
        }
        Ok(settings)
    }

    /// `owner`'s kept programs, oldest first, and whether each comes back.
    pub fn saved_programs(&self, owner: &TerminalOwner) -> Vec<SavedProgram> {
        let Some(resume) = &self.resume else {
            return Vec::new();
        };
        let settings = resume.settings();
        let own_uid = resume.processes.own_uid();
        let hub = self.lock();
        resume
            .store
            .saved()
            .into_iter()
            .filter(|saved| {
                saved.organization == owner.organization && saved.principal == owner.principal
            })
            .map(|saved| SavedProgram {
                comes_back: decide(&settings, own_uid, &saved).is_ok(),
                open: hub
                    .terminals
                    .iter()
                    .any(|live| live.entry.id == saved.id && live.entry.ended.is_none()),
                command: display_command(&saved.record.argv),
                folder: saved.record.folder().map(str::to_owned),
                terminal_id: saved.id,
                title: saved.title,
                saved_at_ms: saved.record.saved_at_ms,
            })
            .collect()
    }

    /// Forget one of `owner`'s kept programs; it does not come back.
    pub fn forget_saved(&self, owner: &TerminalOwner, id: &str) -> Result<(), TerminalError> {
        let resume = self.resume.as_ref().ok_or(TerminalError::Unavailable)?;
        let _writing = resume.writing();
        let held = resume.store.saved().into_iter().any(|saved| {
            saved.id == id
                && saved.organization == owner.organization
                && saved.principal == owner.principal
        });
        if !held {
            return Err(TerminalError::NotFound);
        }
        resume.forget(id);
        let mut hub = self.lock();
        if let Some(live) = hub.terminals.iter_mut().find(|live| live.entry.id == id) {
            live.saved = None;
            if live.entry.resume_command.take().is_some() {
                let owner = live.owner.clone();
                self.announce(&mut hub, &owner);
            }
        }
        Ok(())
    }

    /// Open every kept terminal the settings allow again, by its old id and
    /// name, running its command; forget the rest. Called once, as the
    /// gateway starts. Answers how many came back.
    pub fn resume_saved(&self, host: &dyn TerminalHost) -> usize {
        let Some(resume) = &self.resume else {
            return 0;
        };
        let settings = resume.settings();
        let own_uid = resume.processes.own_uid();
        let mut resumed = 0;
        for saved in resume.store.saved() {
            let shell = host.login_shell();
            let planned =
                decide(&settings, own_uid, &saved).and_then(|()| plan(&saved, host, &shell));
            let (folder, start) = match planned {
                Ok(planned) => planned,
                Err(skip) => {
                    tracing::info!(
                        target: "nessa::terminal",
                        terminal = %saved.id,
                        reason = ?skip,
                        "kept terminal program not resumed"
                    );
                    let _writing = resume.writing();
                    resume.forget(&saved.id);
                    continue;
                }
            };
            let request = OpenRequest {
                owner: TerminalOwner {
                    organization: saved.organization.clone(),
                    principal: saved.principal.clone(),
                },
                request_id: saved.request_id.clone(),
                channel_id: saved.channel_id.clone(),
                conversation_id: saved.conversation_id.clone(),
                folder,
                runs_on: "here".into(),
                kind: TerminalKind::Shell,
                size: GridSize::new(u64::from(saved.cols), u64::from(saved.rows))
                    .unwrap_or(GridSize { cols: 80, rows: 24 }),
                reopen: Some(Reopened {
                    id: saved.id.clone(),
                    title: saved.title.clone(),
                    start,
                }),
            };
            // Marked started before it starts: a gateway that dies while it
            // runs does not start it again on the next start.
            let mut marked = saved.clone();
            marked.record.resumed = true;
            {
                let _writing = resume.writing();
                if let Err(error) = resume.store.save(&marked) {
                    tracing::warn!(target: "nessa::terminal", terminal = %saved.id, %error, "kept terminal not marked; not resumed");
                    resume.forget(&saved.id);
                    continue;
                }
            }
            match self.open(request, shell) {
                Ok(entry) => {
                    resumed += 1;
                    if let Some(live) = self
                        .lock()
                        .terminals
                        .iter_mut()
                        .find(|live| live.entry.id == entry.id)
                    {
                        live.saved = Some(marked.record.clone());
                    }
                    tracing::info!(
                        target: "nessa::terminal",
                        terminal = %entry.id,
                        principal = %marked.principal,
                        program = %marked.record.argv.first().map(String::as_str).unwrap_or(""),
                        "terminal program resumed after restart"
                    );
                }
                Err(error) => {
                    tracing::warn!(target: "nessa::terminal", terminal = %saved.id, ?error, "kept terminal program did not start");
                    let _writing = resume.writing();
                    resume.forget(&saved.id);
                }
            }
        }
        resumed
    }
}

/// Whether the settings let `saved` come back on this account.
fn decide(settings: &ResumeSettings, own_uid: u32, saved: &SavedTerminal) -> Result<(), Skip> {
    if !settings.enabled {
        return Err(Skip::Off);
    }
    if saved.record.uid != own_uid || own_uid == 0 {
        return Err(Skip::OtherAccount);
    }
    if saved.record.resumed {
        return Err(Skip::StartedOnce);
    }
    if !settings.allows(&saved.record.argv, saved.record.folder()) {
        return Err(Skip::NotAllowed);
    }
    Ok(())
}

/// Where a kept terminal opens and what it runs. A folder the program named
/// that is gone means it does not come back; one the system saw that is
/// gone falls back to the terminal's own folder.
fn plan(
    saved: &SavedTerminal,
    host: &dyn TerminalHost,
    shell: &std::path::Path,
) -> Result<(std::path::PathBuf, ResumeStart), Skip> {
    let folder = host
        .folder(std::path::Path::new(&saved.folder))
        .ok_or(Skip::FolderGone)?;
    let cwd = match (&saved.record.cwd, &saved.record.owner_cwd) {
        (Some(cwd), _) => host
            .folder(std::path::Path::new(cwd))
            .ok_or(Skip::FolderGone)?,
        (None, Some(cwd)) => host
            .folder(std::path::Path::new(cwd))
            .unwrap_or_else(|| folder.clone()),
        (None, None) => folder.clone(),
    };
    let start = ResumeStart {
        argv: saved.record.argv.clone(),
        cwd,
    };
    start.shell_arguments(shell).ok_or(Skip::Unstartable)?;
    Ok((folder, start))
}
