//! Lightweight GPUI host for the cursor-centred Actions Ring.
//!
//! This process is a pure IPC client. The agent owns HID++, session validation,
//! haptic output, and action execution; the overlay only renders the
//! agent-snapshotted actions and reports hover/activate/cancel interactions.

#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

// `t!` resolves against a backend the invoking crate must generate itself, so
// both binaries expand `i18n!` over the one catalog in `openlogi-ui` — the same
// crate this one already depends on for locale negotiation.
rust_i18n::i18n!("../openlogi-ui/locales", fallback = "en");

mod ipc;
mod platform;
mod presenter;
mod ring;
mod session;

use std::sync::Arc;

use anyhow::Result;
use gpui::{AppContext as _, WindowHandle};
use tracing::warn;

use openlogi_core::action_ring::DISPLAY_LIFETIME;
use openlogi_ipc::{ActionRingInvocation, PresenterObservation};

use crate::ipc::OverlayCommand;
use crate::platform::RingPlacement;
use crate::presenter::{PresenterView, presenter_window_options};
use crate::ring::RingView;
use crate::session::{ClickAwaySession, claim_the_role, spawn_click_away_dismissal};

enum OverlayEvent {
    RingClosed,
    Ring(Option<ActionRingInvocation>),
    PresenterClosed,
    Presenter(PresenterObservation),
}

#[expect(
    clippy::too_many_lines,
    reason = "the overlay owns two level-triggered windows in one event loop"
)]
fn main() -> Result<()> {
    openlogi_core::logging::init_stderr();

    openlogi_core::locale::activate(None);
    // Held for the whole run: dropping it hands the role to the replacement.
    let _tenancy = claim_the_role()?;
    let ipc::Handle {
        mut invocations,
        mut presenter,
        commands,
    } = ipc::spawn();

    let mut app = gpui_platform::application().with_assets(openlogi_ui::action_icons::ActionIcons);
    app = app.with_quit_mode(gpui::QuitMode::Explicit);
    app.run(move |cx| {
        platform::configure_application();
        let live_session = Arc::new(ClickAwaySession::new());
        spawn_click_away_dismissal(cx, Arc::clone(&live_session));
        cx.spawn(async move |cx| {
            let mut ring_handle: Option<WindowHandle<RingView>> = None;
            let mut presenter_handles: Vec<WindowHandle<PresenterView>> = Vec::new();
            loop {
                let observed = tokio::select! {
                    value = invocations.recv() => value.map_or(OverlayEvent::RingClosed, OverlayEvent::Ring),
                    value = presenter.recv() => value.map_or(OverlayEvent::PresenterClosed, OverlayEvent::Presenter),
                };
                match observed {
                OverlayEvent::RingClosed | OverlayEvent::PresenterClosed => break,
                OverlayEvent::Ring(observed) => {
                // No ring is what a dismissal looks like: close whatever is
                // showing and open nothing. The agent has already forgotten the
                // session, so there is nothing to acknowledge either.
                let Some(invocation) = observed else {
                    cx.update(|cx| {
                        if let Some(handle) = ring_handle.take() {
                            let _ = handle.update(cx, |_, window, _| window.remove_window());
                        }
                    });
                    continue;
                };
                openlogi_core::locale::activate(invocation.language.as_deref());
                cx.update(|cx| {
                    if let Some(handle) = ring_handle.take() {
                        let _ = handle.update(cx, |_, window, _| window.remove_window());
                    }
                    let placement = match RingPlacement::capture(cx) {
                        Ok(placement) => placement,
                        Err(error) => {
                            warn!(%error, "could not locate Actions Ring display");
                            let _ = commands.send(OverlayCommand::Cancel {
                                session_id: invocation.session_id,
                            });
                            return;
                        }
                    };
                    let commands = commands.clone();
                    let timeout_commands = commands.clone();
                    let session_id = invocation.session_id;
                    match cx.open_window(placement.window_options(), |_, cx| {
                        cx.new(|_| RingView::new(invocation, commands, &live_session))
                    }) {
                        Ok(handle) => {
                            if let Err(error) = handle
                                .update(cx, |_, window, _| placement.show(window))
                                .and_then(std::convert::identity)
                            {
                                warn!(%error, "could not position Actions Ring window");
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                                let _ =
                                    timeout_commands.send(OverlayCommand::Cancel { session_id });
                                return;
                            }
                            ring_handle = Some(handle);
                            platform::configure_windows();
                            cx.spawn(async move |cx| {
                                cx.background_executor().timer(DISPLAY_LIFETIME).await;
                                if handle
                                    .update(cx, |_, window, _| window.remove_window())
                                    .is_ok()
                                {
                                    let _ = timeout_commands
                                        .send(OverlayCommand::Cancel { session_id });
                                }
                            })
                            .detach();
                        }
                        Err(error) => warn!(%error, "could not open Actions Ring window"),
                    }
                });
                }
                OverlayEvent::Presenter(observed) => {
                    cx.update(|cx| match observed.overlay {
                        Some(overlay) => {
                            if presenter_handles.is_empty() {
                                for (options, display) in presenter_window_options(cx) {
                                    let overlay = overlay.clone();
                                    match cx.open_window(options, |_, cx| {
                                        cx.new(|_| PresenterView::new(overlay, display))
                                    }) {
                                        Ok(handle) => {
                                            presenter_handles.push(handle);
                                            cx.spawn(async move |cx| {
                                                loop {
                                                    cx.background_executor()
                                                        .timer(std::time::Duration::from_millis(16))
                                                        .await;
                                                    if handle.update(cx, |_, _, cx| cx.notify()).is_err() {
                                                        break;
                                                    }
                                                }
                                            })
                                            .detach();
                                        }
                                        Err(error) => warn!(%error, "could not open presenter overlay window"),
                                    }
                                }
                                platform::configure_presenter_window();
                            } else {
                                for handle in &presenter_handles {
                                    let overlay = overlay.clone();
                                    let _ = handle.update(cx, |view, _, cx| view.update(overlay, cx));
                                }
                            }
                        }
                        None => {
                            for handle in presenter_handles.drain(..) {
                                let _ = handle.update(cx, |_, window, _| window.remove_window());
                            }
                        }
                    });
                }
                }
            }
        })
        .detach();
    });
    Ok(())
}

#[cfg(test)]
mod tests;
