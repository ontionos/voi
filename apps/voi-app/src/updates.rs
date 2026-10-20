//! Asking now and then whether a newer Voi is out (`voi_core::update`), so the
//! menu can offer it: "Update Available", just above Quit, on the Mac and on
//! Windows alike. Choosing it downloads this system's file in the browser.

use std::time::Duration;

use crate::state::Shared;

/// The first ask, once the app has settled after starting.
const FIRST: Duration = Duration::from_secs(10);
/// And then: an app left running for days still hears of a release.
const EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// This build's number: commits plus `config/build-base`, given by the build
/// scripts. 0 for a plain `cargo build`, which never asks.
pub fn this_build() -> u64 {
    env!("VOI_BUILD").parse().unwrap_or(0)
}

/// The file people on this system download.
const EXTENSION: &str = if cfg!(windows) { ".exe" } else { ".dmg" };

/// Keep `shared.update` current; `changed` is called whenever it changes.
pub fn watch(shared: Shared, changed: impl Fn() + Send + 'static) {
    let mine = this_build();
    if mine == 0 {
        tracing::info!("this build has no number: not asking for updates");
        return;
    }
    crate::engine::runtime().spawn(async move {
        tokio::time::sleep(FIRST).await;
        loop {
            match voi_core::update::check(mine, EXTENSION).await {
                Ok(found) => {
                    let mut s = shared.lock().unwrap();
                    if s.update != found {
                        match &found {
                            Some(u) => tracing::info!(build = u.build, mine, "update available"),
                            None => tracing::info!(mine, "up to date"),
                        }
                        s.update = found;
                        drop(s);
                        changed();
                    }
                }
                // Offline, GitHub down, a limit reached: the next ask may work.
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "could not ask for updates"),
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

/// The menu's title for it.
pub fn title(update: &voi_core::update::Update) -> String {
    format!("Update Available: {}", update.label)
}
