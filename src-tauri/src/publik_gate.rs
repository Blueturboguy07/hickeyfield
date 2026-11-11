//! Sign in with publik: the gate's screens for the Tauri shell.
//!
//! The main window starts hidden (`visible: false` in tauri.conf.json). The gate runs on its
//! own thread from the setup hook, draws each screen of `publik::ensure_signed_in` as a native
//! message dialog, and shows the main window only after the person is signed in. There is no
//! other way past it. `PUBLIK_SELF_TEST=1` makes the module print its one line and exit instead.

use crate::publik::{self, GateUi, Screen};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

/// One native message dialog per screen: `screen.title`, `screen.body` and `screen.note`, with
/// one button for each of `screen.buttons`. The dialog plugin runs it on the main thread and
/// blocks only this thread.
struct DialogUi {
    app: AppHandle,
}

impl GateUi for DialogUi {
    fn show(&mut self, s: &Screen, done: Arc<AtomicBool>) -> Option<&'static str> {
        let buttons = match s.buttons.as_slice() {
            [(_, a), (_, b)] => MessageDialogButtons::OkCancelCustom(a.to_string(), b.to_string()),
            [(_, a)] => MessageDialogButtons::OkCustom(a.to_string()),
            _ => MessageDialogButtons::Ok,
        };
        let detail = match s.note {
            Some(note) => format!("{}\n\n{note}", s.body),
            None => s.body.clone(),
        };
        let first = self
            .app
            .dialog()
            .message(detail)
            .title(s.title)
            .kind(MessageDialogKind::Info)
            .buttons(buttons)
            .blocking_show();
        // The poll saw the sign-in (or the plan) while the dialog was open: the press only
        // closed it, so the gate reads the wallet again instead of opening the browser twice.
        if done.load(Ordering::SeqCst) {
            return None;
        }
        match (first, s.buttons.as_slice()) {
            (true, [(id, _), ..]) => Some(*id),
            (false, [_, (id, _)]) => Some(*id),
            // A one-button screen closed another way (Escape): the gate shows it again.
            _ => None,
        }
    }

    fn open_url(&mut self, url: &str) {
        if let Err(e) = self.app.opener().open_url(url, None::<&str>) {
            tracing::warn!("publik: could not open the browser: {e}");
        }
    }
}

/// Called first in the setup hook. Runs the gate on a thread, then shows the main window.
pub fn run(app: AppHandle) {
    let spawned = std::thread::Builder::new()
        .name("publik-gate".into())
        .spawn({
            let app = app.clone();
            move || {
                let mut ui = DialogUi { app: app.clone() };
                let (_key, _wallet, choice) = publik::ensure_signed_in(&mut ui);
                tracing::info!(
                    "publik: signed in; AI choice: {}",
                    choice.as_deref().unwrap_or("none")
                );
                show_main(&app);
            }
        });
    if let Err(e) = spawned {
        // No thread, no gate: the app must still open rather than stay invisible.
        tracing::error!("publik: could not start the sign-in gate: {e}");
        show_main(&app);
    }
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
