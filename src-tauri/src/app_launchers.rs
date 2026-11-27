//! Each installed app as an app of its own: one launcher per app, run by its
//! own copy of `nessa-app-host` (`src/bin/nessa-app-host.rs`).
//!
//! ```text
//! macOS   ~/Applications/Nessa Apps/<app>.app
//!           Contents/Info.plist            its own name, bundle id and icon
//!           Contents/MacOS/<app>           a copy of nessa-app-host
//!           Contents/Resources/AppIcon.icns   from the package's PNG icon
//!           Contents/Resources/nessa-app/  app.json, running.lock, running.pid
//! Linux   <config>/installed-apps/<app>/   app.json, <app>, icon.png, locks
//!         $XDG_DATA_HOME/applications/so.nessa.app.<app>.desktop
//! Windows <config>/installed-apps/<app>/   app.json, <app>.exe, locks
//! ```
//!
//! On macOS a process's Dock tile, name and icon come from the bundle its
//! executable sits in, so each app's bundle holds a copy of the host rather
//! than pointing at Nessa's. A copy also gives each app its own taskbar group
//! on Windows and its own window class on Linux.
//!
//! The gateway stays the authority on what is installed. A launcher is a copy
//! of one verified package that its host verifies again before drawing, so
//! the app opens from Finder, Launchpad or the menu with Nessa closed. Who is
//! running is the launcher's own running lock: held is running, and it goes
//! with the process however the app was opened or ended.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use fs2::FileExt;
use nessa_app_package::{
    launcher_record, VerifiedPackage, LAUNCHER_RECORD, RUNNING_LOCK, RUNNING_PID,
};

/// The host binary, beside Nessa's own executable in every build.
pub const APP_HOST: &str = "nessa-app-host";

/// How long Stop waits for an app's process to end.
const STOP_WAIT: Duration = Duration::from_secs(5);

/// How a launcher is laid out on this computer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// A macOS app bundle, `<app>.app`.
    Bundle,
    /// A folder holding the record and the host, named `<app>`.
    Folder,
}

/// Where launchers go and what runs them.
pub struct AppLaunchers {
    layout: Layout,
    /// The folder every launcher sits in.
    root: PathBuf,
    /// Linux only: where desktop entries go, so the app is in the menu.
    menu: Option<PathBuf>,
    /// The `nessa-app-host` each launcher gets a copy of.
    host: PathBuf,
}

impl AppLaunchers {
    pub fn new(layout: Layout, root: PathBuf, menu: Option<PathBuf>, host: PathBuf) -> Self {
        Self {
            layout,
            root,
            menu,
            host,
        }
    }

    /// The launchers for this platform: `stage` keeps a development Nessa's
    /// apps apart from the shipped one's.
    pub fn native(home: &Path, config: &Path, data: &Path, stage: &str) -> io::Result<Self> {
        let host = std::env::current_exe()?
            .parent()
            .map(|folder| folder.join(format!("{APP_HOST}{}", std::env::consts::EXE_SUFFIX)))
            .ok_or_else(|| io::Error::other("Nessa's executable has no folder"))?;
        let launchers = if cfg!(target_os = "macos") {
            let folder = if stage == "prod" {
                "Nessa Apps".to_owned()
            } else {
                format!("Nessa Apps ({stage})")
            };
            Self::new(
                Layout::Bundle,
                home.join("Applications").join(folder),
                None,
                host,
            )
        } else {
            Self::new(
                Layout::Folder,
                config.join("installed-apps"),
                cfg!(target_os = "linux").then(|| data.join("applications")),
                host,
            )
        };
        Ok(launchers)
    }

    /// The launcher itself: the bundle, or the folder.
    fn launcher(&self, app: &str) -> PathBuf {
        match self.layout {
            Layout::Bundle => self.root.join(format!("{app}.app")),
            Layout::Folder => self.root.join(app),
        }
    }

    /// The launcher's folder: its record and running lock.
    fn folder(&self, app: &str) -> PathBuf {
        match self.layout {
            Layout::Bundle => self
                .launcher(app)
                .join("Contents")
                .join("Resources")
                .join("nessa-app"),
            Layout::Folder => self.launcher(app),
        }
    }

    fn executable(&self, app: &str) -> PathBuf {
        match self.layout {
            Layout::Bundle => self.launcher(app).join("Contents").join("MacOS").join(app),
            Layout::Folder => self
                .launcher(app)
                .join(format!("{app}{}", std::env::consts::EXE_SUFFIX)),
        }
    }

    fn menu_entry(&self, app: &str) -> Option<PathBuf> {
        self.menu
            .as_ref()
            .map(|menu| menu.join(format!("so.nessa.app.{app}.desktop")))
    }

    /// Write `package`'s launcher, replacing one that holds anything else.
    /// One that already holds exactly this package and host is left as it is,
    /// so placing a running app again does not pull its files away.
    pub fn place(&self, package: &VerifiedPackage) -> Result<(), String> {
        let app = package.manifest().app_id().as_str();
        let record = launcher_record(package)
            .map_err(|refusal| format!("could not write {app}'s launcher: {refusal:?}"))?;
        let (same_record, same_host) = self.holds(app, &record);
        if same_record && (same_host || self.is_running(app)) {
            // A running app keeps its files; a new host waits for its next run.
            return Ok(());
        }
        if self.is_running(app) {
            return Err(format!("{app} is open with other files; stop it first"));
        }
        fs::create_dir_all(&self.root).map_err(|error| place_error(app, &error))?;
        let staging = self
            .root
            .join(format!(".{app}.placing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        let written = self.write_launcher(package, &record, &staging);
        let swapped = written.and_then(|()| swap_in(&staging, &self.launcher(app)));
        if let Err(error) = swapped {
            let _ = fs::remove_dir_all(&staging);
            return Err(place_error(app, &error));
        }
        if let Some(entry) = self.menu_entry(app) {
            let written = entry
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| {
                    fs::write(
                        &entry,
                        desktop_entry(app, &self.executable(app), &self.folder(app)),
                    )
                });
            written.map_err(|error| place_error(app, &error))?;
        }
        Ok(())
    }

    /// Whether `app`'s launcher holds exactly `record`, and the host as built.
    fn holds(&self, app: &str, record: &[u8]) -> (bool, bool) {
        let same_record =
            fs::read(self.folder(app).join(LAUNCHER_RECORD)).is_ok_and(|held| held == record);
        let same_host = match (fs::read(self.executable(app)), fs::read(&self.host)) {
            (Ok(held), Ok(built)) => held == built,
            _ => false,
        };
        (same_record, same_host)
    }

    /// Write the launcher whole into `staging`, laid out as it will be.
    fn write_launcher(
        &self,
        package: &VerifiedPackage,
        record: &[u8],
        staging: &Path,
    ) -> io::Result<()> {
        let app = package.manifest().app_id().as_str();
        let icon = package
            .manifest()
            .icon()
            .filter(|path| path.ends_with(".png"))
            .and_then(|path| package.asset(path));
        let (folder, executable) = match self.layout {
            Layout::Bundle => {
                let contents = staging.join("Contents");
                let resources = contents.join("Resources");
                fs::create_dir_all(contents.join("MacOS"))?;
                fs::create_dir_all(&resources)?;
                let icns = icon.and_then(icns_from_png);
                if let Some(icns) = &icns {
                    fs::write(resources.join("AppIcon.icns"), icns)?;
                }
                fs::write(
                    contents.join("Info.plist"),
                    info_plist(app, package.manifest().package_version(), icns.is_some()),
                )?;
                (
                    resources.join("nessa-app"),
                    contents.join("MacOS").join(app),
                )
            }
            Layout::Folder => {
                if let Some(png) = icon {
                    fs::create_dir_all(staging)?;
                    fs::write(staging.join("icon.png"), png)?;
                }
                (
                    staging.to_owned(),
                    staging.join(format!("{app}{}", std::env::consts::EXE_SUFFIX)),
                )
            }
        };
        fs::create_dir_all(&folder)?;
        fs::write(folder.join(LAUNCHER_RECORD), record)?;
        fs::copy(&self.host, executable).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("copy {}: {error}", self.host.display()),
            )
        })?;
        Ok(())
    }

    /// Remove `app`'s launcher and its menu entry; nothing when there is none.
    pub fn remove(&self, app: &str) -> Result<(), String> {
        if self.is_running(app) {
            return Err(format!("{app} is still open"));
        }
        if let Some(entry) = self.menu_entry(app) {
            remove_file(&entry).map_err(|error| remove_error(app, &error))?;
        }
        match fs::remove_dir_all(self.launcher(app)) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(remove_error(app, &error)),
            _ => Ok(()),
        }
    }

    /// Open `app` as the system opens any app: through Launch Services on
    /// macOS, so a second Open brings the running one forward.
    pub fn launch(&self, app: &str) -> Result<(), String> {
        let executable = self.executable(app);
        if !executable.is_file() {
            return Err(format!("{app} has no launcher"));
        }
        let mut command = match self.layout {
            Layout::Bundle => {
                let mut open = Command::new("open");
                open.arg(self.launcher(app));
                open
            }
            Layout::Folder => {
                let mut host = Command::new(&executable);
                host.arg("--app").arg(self.folder(app));
                host
            }
        };
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("could not open {app}: {error}"))?;
        // Reap it, so an ended app leaves no zombie behind.
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    /// Whether `app`'s host holds its running lock now.
    pub fn is_running(&self, app: &str) -> bool {
        held(&self.folder(app).join(RUNNING_LOCK))
    }

    /// The apps running now, in name order.
    pub fn running(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut running: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .filter_map(|name| match self.layout {
                Layout::Bundle => name.strip_suffix(".app").map(str::to_owned),
                Layout::Folder => Some(name),
            })
            .filter(|app| self.is_running(app))
            .collect();
        running.sort();
        running
    }

    /// End `app`'s process, and wait for it to go. Nothing when it is not
    /// running. The id is read only while the lock is held, so a stale id
    /// from an earlier run is never signalled.
    pub fn stop(&self, app: &str) -> Result<(), String> {
        if !self.is_running(app) {
            return Ok(());
        }
        let pid = fs::read_to_string(self.folder(app).join(RUNNING_PID))
            .ok()
            .and_then(|pid| pid.trim().parse::<u32>().ok())
            .ok_or_else(|| format!("could not stop {app}: it named no process"))?;
        end_process(pid).map_err(|error| format!("could not stop {app}: {error}"))?;
        let deadline = Instant::now() + STOP_WAIT;
        while self.is_running(app) {
            if Instant::now() > deadline {
                return Err(format!("{app} did not stop"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }
}

/// Whether another process holds `lock`. Probing takes it for an instant;
/// a host starting just then retries (`nessa-app-host`'s `LOCK_TRIES`).
fn held(lock: &Path) -> bool {
    let Ok(file) = File::options().write(true).open(lock) else {
        return false;
    };
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            false
        }
        Err(error) => error.kind() == fs2::lock_contended_error().kind(),
    }
}

#[cfg(unix)]
fn end_process(pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    // SAFETY: `kill` only sends a signal; the id was read while its owner
    // held the running lock.
    if unsafe { libc::kill(pid, libc::SIGTERM) } == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn end_process(pid: u32) -> io::Result<()> {
    // Without /F, taskkill asks the window to close, as its close button does.
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("taskkill ended {status}")))
    }
}

/// Put `staging` where `launcher` is, keeping the old one until the new one
/// is in place.
fn swap_in(staging: &Path, launcher: &Path) -> io::Result<()> {
    let parent = launcher.parent().unwrap_or(Path::new("."));
    let name = launcher
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let old = parent.join(format!(".{name}.old-{}", std::process::id()));
    let _ = fs::remove_dir_all(&old);
    let had = match fs::rename(launcher, &old) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => return Err(error),
    };
    if let Err(error) = fs::rename(staging, launcher) {
        if had {
            let _ = fs::rename(&old, launcher);
        }
        return Err(error);
    }
    if had {
        let _ = fs::remove_dir_all(&old);
    }
    Ok(())
}

fn remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

fn place_error(app: &str, error: &io::Error) -> String {
    format!("could not write {app}'s launcher: {error}")
}

fn remove_error(app: &str, error: &io::Error) -> String {
    format!("could not remove {app}'s launcher: {error}")
}

fn xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The bundle's Info.plist: its own identity, so the Dock and Finder treat it
/// as an app of its own.
fn info_plist(app: &str, version: &str, icon: bool) -> String {
    let app = xml_text(app);
    let version = xml_text(version);
    let icon = if icon {
        "  <key>CFBundleIconFile</key>\n  <string>AppIcon</string>\n"
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>{app}</string>
  <key>CFBundleExecutable</key>
  <string>{app}</string>
{icon}  <key>CFBundleIdentifier</key>
  <string>so.nessa.installed.{app}</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>{app}</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>{version}</string>
  <key>CFBundleVersion</key>
  <string>{version}</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
"#
    )
}

/// An `.icns` holding the PNG as is, when it is a square of a size the
/// format names. Any other image leaves the bundle with the system's icon.
fn icns_from_png(png: &[u8]) -> Option<Vec<u8>> {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    if png.len() < 24 || !png.starts_with(SIGNATURE) || &png[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes(png[16..20].try_into().ok()?);
    let height = u32::from_be_bytes(png[20..24].try_into().ok()?);
    let kind: &[u8; 4] = match (width == height).then_some(width)? {
        16 => b"icp4",
        32 => b"icp5",
        64 => b"icp6",
        128 => b"ic07",
        256 => b"ic08",
        512 => b"ic09",
        1024 => b"ic10",
        _ => return None,
    };
    let entry = u32::try_from(png.len() + 8).ok()?;
    let total = entry.checked_add(8)?;
    let mut icns = Vec::with_capacity(total as usize);
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&total.to_be_bytes());
    icns.extend_from_slice(kind);
    icns.extend_from_slice(&entry.to_be_bytes());
    icns.extend_from_slice(png);
    Some(icns)
}

/// A freedesktop menu entry for the app. Paths are quoted as the
/// specification asks, so a space in the user's folder survives.
fn desktop_entry(app: &str, executable: &Path, folder: &Path) -> String {
    let quote = |path: &Path| {
        let text = path.to_string_lossy();
        let mut quoted = String::from("\"");
        for character in text.chars() {
            if matches!(character, '"' | '`' | '$' | '\\') {
                quoted.push('\\');
            }
            quoted.push(character);
        }
        quoted.push('"');
        // A literal `%` is doubled; `\` was escaped above and is escaped
        // again by the string level of the format.
        quoted.replace('%', "%%").replace('\\', "\\\\")
    };
    let icon = folder.join("icon.png");
    let icon = if icon.is_file() {
        format!("Icon={}\n", icon.to_string_lossy())
    } else {
        String::new()
    };
    format!(
        "[Desktop Entry]\nType=Application\nName={app}\nComment=An app installed by Nessa\nExec={} --app {}\n{icon}Terminal=false\nCategories=Utility;\nStartupWMClass={app}\n",
        quote(executable),
        quote(folder),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_app_package::{read_launcher_record, scaffold_package};

    fn starter(app: &str) -> VerifiedPackage {
        scaffold_package(app.to_owned(), "1.0.0".to_owned()).unwrap()
    }

    fn launchers(layout: Layout) -> (tempfile::TempDir, AppLaunchers) {
        let dir = tempfile::tempdir().unwrap();
        let host = dir.path().join("nessa-app-host");
        fs::write(&host, b"host build 1").unwrap();
        let menu = (layout == Layout::Folder).then(|| dir.path().join("applications"));
        let launchers = AppLaunchers::new(layout, dir.path().join("apps"), menu, host);
        (dir, launchers)
    }

    #[test]
    fn a_bundle_is_an_app_of_its_own_with_the_verified_package_inside() {
        let (dir, launchers) = launchers(Layout::Bundle);
        let package = starter("notes");
        launchers.place(&package).unwrap();
        let bundle = dir.path().join("apps").join("notes.app").join("Contents");
        let plist = fs::read_to_string(bundle.join("Info.plist")).unwrap();
        assert!(plist.contains("<string>so.nessa.installed.notes</string>"));
        assert!(plist.contains("<key>CFBundleExecutable</key>\n  <string>notes</string>"));
        assert!(plist.contains("<key>CFBundleIconFile</key>"));
        assert_eq!(
            fs::read(bundle.join("MacOS").join("notes")).unwrap(),
            b"host build 1"
        );
        let icns = fs::read(bundle.join("Resources").join("AppIcon.icns")).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(&icns[8..12], b"ic08", "the starter's icon is 256 px");
        let record = fs::read(
            bundle
                .join("Resources")
                .join("nessa-app")
                .join(LAUNCHER_RECORD),
        )
        .unwrap();
        assert_eq!(
            read_launcher_record(&record).unwrap().digest(),
            package.digest()
        );
        assert!(launchers.running().is_empty());
    }

    #[test]
    fn a_folder_launcher_is_in_the_menu_and_goes_with_its_entry() {
        let (dir, launchers) = launchers(Layout::Folder);
        launchers.place(&starter("notes")).unwrap();
        launchers.place(&starter("calendar")).unwrap();
        let folder = dir.path().join("apps").join("notes");
        assert!(folder.join(LAUNCHER_RECORD).is_file());
        assert!(folder.join("icon.png").is_file());
        let entry = dir
            .path()
            .join("applications")
            .join("so.nessa.app.notes.desktop");
        let text = fs::read_to_string(&entry).unwrap();
        assert!(text.contains("Name=notes\n"));
        assert!(text.contains(" --app \""));
        assert!(text.contains("StartupWMClass=notes\n"));

        launchers.remove("notes").unwrap();
        assert!(!folder.exists());
        assert!(!entry.exists());
        assert!(dir.path().join("apps").join("calendar").is_dir());
        // Removing again is nothing.
        launchers.remove("notes").unwrap();
    }

    #[test]
    fn placing_the_same_package_again_leaves_the_launcher_alone() {
        let (dir, launchers) = launchers(Layout::Folder);
        let package = starter("notes");
        launchers.place(&package).unwrap();
        let record = dir.path().join("apps").join("notes").join(LAUNCHER_RECORD);
        let before = fs::metadata(&record).unwrap().modified().unwrap();
        launchers.place(&package).unwrap();
        assert_eq!(fs::metadata(&record).unwrap().modified().unwrap(), before);

        // A new Nessa brings a new host: the launcher takes it.
        fs::write(dir.path().join("nessa-app-host"), b"host build 2").unwrap();
        launchers.place(&package).unwrap();
        assert_eq!(
            fs::read(dir.path().join("apps").join("notes").join("notes")).unwrap(),
            b"host build 2"
        );
        // Nothing is left half-written beside it.
        let names: Vec<_> = fs::read_dir(dir.path().join("apps"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["notes"]);
    }

    #[test]
    fn running_is_whoever_holds_the_lock() {
        let (dir, launchers) = launchers(Layout::Folder);
        launchers.place(&starter("notes")).unwrap();
        launchers.place(&starter("calendar")).unwrap();
        assert!(launchers.running().is_empty());

        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.path().join("apps").join("notes").join(RUNNING_LOCK))
            .unwrap();
        lock.lock_exclusive().unwrap();
        // Probing from here is another open file description, as another
        // process's would be.
        assert_eq!(launchers.running(), ["notes"]);
        assert!(launchers.remove("notes").is_err(), "an open app stays");
        assert!(
            launchers.place(&starter("notes")).is_ok(),
            "the same package placed again is nothing"
        );
        FileExt::unlock(&lock).unwrap();
        assert!(launchers.running().is_empty());
        // Stop of an app that is not running is nothing.
        launchers.stop("calendar").unwrap();
    }

    #[test]
    fn an_app_without_a_launcher_does_not_open() {
        let (_dir, launchers) = launchers(Layout::Folder);
        assert!(launchers.launch("notes").is_err());
    }

    #[test]
    fn an_icon_becomes_an_icns_only_when_it_is_a_named_square() {
        let png = |width: u32, height: u32| {
            let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
            bytes.extend_from_slice(&width.to_be_bytes());
            bytes.extend_from_slice(&height.to_be_bytes());
            bytes
        };
        let icns = icns_from_png(&png(512, 512)).unwrap();
        assert_eq!(&icns[8..12], b"ic09");
        assert_eq!(
            u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize,
            icns.len()
        );
        assert!(icns_from_png(&png(512, 256)).is_none());
        assert!(icns_from_png(&png(300, 300)).is_none());
        assert!(icns_from_png(b"GIF89a").is_none());
    }

    #[test]
    fn names_reach_the_plist_and_menu_as_text() {
        let plist = info_plist("notes", "1.0 <beta> & \"rc\"", false);
        assert!(plist.contains("1.0 &lt;beta&gt; &amp; &quot;rc&quot;"));
        assert!(!plist.contains("CFBundleIconFile"));
        let entry = desktop_entry(
            "notes",
            Path::new("/home/a b/apps/notes/notes"),
            Path::new("/home/a b/apps/notes"),
        );
        assert!(
            entry.contains("Exec=\"/home/a b/apps/notes/notes\" --app \"/home/a b/apps/notes\"\n")
        );
        let odd = desktop_entry("notes", Path::new("/x/$HOME%u"), Path::new("/x"));
        assert!(odd.contains(r#"Exec="/x/\\$HOME%%u""#));
    }
}
