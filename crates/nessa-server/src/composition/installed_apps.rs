//! The owner's installed apps, composed once per run: the private SQLite
//! store at `<namespace>/conversations/installed-apps.sqlite3` and the audit
//! beside the voice one.
use std::{path::Path, sync::Arc};

use super::local_auth::{conversation_root, SystemClock};
use crate::installed_apps::application::InstalledApps;
use crate::installed_apps::infrastructure::{DurableAppsAudit, SqliteAppStore};

/// The installed apps kept under `namespace`, or `None` — logged — when the
/// store cannot be opened. Without them the apps methods answer
/// `apps_not_configured`, and nothing installed is lost.
pub(super) fn compose(namespace: &Path) -> Option<Arc<InstalledApps>> {
    let root = conversation_root(namespace);
    if let Err(error) = nessa_local_storage::create_directory(&root) {
        tracing::error!(%error, "installed apps are off this run: the data folder could not be made");
        return None;
    }
    let store = match SqliteAppStore::open(&root.join("installed-apps.sqlite3")) {
        Ok(store) => store,
        Err(error) => {
            tracing::error!(
                ?error,
                "installed apps are off this run: the store could not be opened"
            );
            return None;
        }
    };
    Some(Arc::new(InstalledApps::new(
        Box::new(store),
        Arc::new(DurableAppsAudit::new(
            root.join("audit").join("apps"),
            Arc::new(SystemClock),
        )),
    )))
}
