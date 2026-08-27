//! Host-side runtime for Logitech Spotlight visual effects and timer state.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use openlogi_core::hid::{PresenterEffect, PresenterSettings, PresenterTimerMode};
use openlogi_hid::DeviceRoute;
use openlogi_ipc::{Generation, OBSERVE_HOLD, PresenterObservation, PresenterOverlay};
use tokio::sync::{Notify, mpsc};
use tracing::debug;

/// One firmware vibration request.
#[derive(Clone, Debug)]
pub struct PresenterHapticRequest {
    /// Presenter that should vibrate.
    pub route: DeviceRoute,
    /// User-facing intensity percentage (`0..=100`).
    pub intensity: u8,
}

#[derive(Clone)]
struct RuntimeState {
    generation: Generation,
    configured: PresenterSettings,
    selected_effect: PresenterEffect,
    active_route: Option<DeviceRoute>,
    effect: Option<PresenterEffect>,
    timer_deadline: Option<Instant>,
    timer_current_time: bool,
    timer_started: bool,
    frozen_position: Option<(i32, i32)>,
    pointer_held: bool,
}

/// Shared host-rendered Spotlight state.
#[derive(Clone)]
pub struct PresenterManager {
    state: Arc<Mutex<RuntimeState>>,
    changed: Arc<Notify>,
    haptic_sender: Arc<Mutex<Option<mpsc::UnboundedSender<PresenterHapticRequest>>>>,
}

impl Default for PresenterManager {
    fn default() -> Self {
        let configured = PresenterSettings::default();
        Self {
            state: Arc::new(Mutex::new(RuntimeState {
                generation: 1,
                selected_effect: configured_effect(&configured),
                configured,
                active_route: None,
                effect: None,
                timer_deadline: None,
                timer_current_time: false,
                timer_started: false,
                frozen_position: None,
                pointer_held: false,
            })),
            changed: Arc::new(Notify::new()),
            haptic_sender: Arc::new(Mutex::new(None)),
        }
    }
}

impl PresenterManager {
    /// Show one visual effect until it is replaced or cancelled.
    pub fn show(&self, effect: PresenterEffect) {
        self.set_effect(Some(effect));
    }

    /// Toggle one visual effect when invoked as an ordinary action.
    pub fn toggle(&self, effect: PresenterEffect) {
        let active = self.lock().effect;
        self.set_effect((active != Some(effect)).then_some(effect));
    }

    /// Update settings used by all presenter actions.
    pub fn set_configured_settings(&self, mut settings: PresenterSettings) {
        settings.enabled_effects = settings.normalized_enabled_effects();
        settings.effect_order = settings.normalized_effect_order();
        settings.effect_size = settings.effect_size.clamp(50, 200);
        settings.effect_contrast = settings.effect_contrast.clamp(25, 100);
        settings.spotlight_radius = settings.spotlight_radius.clamp(40, 320);
        settings.magnifier_radius = settings.magnifier_radius.clamp(40, 260);
        settings.magnification = settings.normalized_magnification();
        settings.vibration_intensity = settings.vibration_intensity.min(100);
        settings.timer_mode = settings.normalized_timer_mode();

        let mut state = self.lock();
        if state.configured == settings {
            return;
        }
        let configured_effect_changed = state.configured.effect != settings.effect;
        state.configured = settings;
        if configured_effect_changed || !state.configured.effect_enabled(state.selected_effect) {
            state.selected_effect = configured_effect(&state.configured);
        }
        if state.effect.is_some() || state.timer_started {
            bump_locked(&mut state);
            drop(state);
            self.changed.notify_waiters();
        }
    }

    /// Attach the agent's firmware-haptic worker to presenter notifications.
    pub fn set_haptic_sender(&self, sender: mpsc::UnboundedSender<PresenterHapticRequest>) {
        *self
            .haptic_sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(sender);
    }

    /// Remember the route that generated the current presenter action.
    pub fn set_active_route(&self, route: DeviceRoute) {
        self.lock().active_route = Some(route);
    }

    /// Emit the one-shot low-battery vibration requested by an inventory edge.
    /// The caller owns edge de-duplication because inventory is device-scoped,
    /// while this manager owns the firmware-haptic channel.
    pub fn alert_low_battery(&self, route: DeviceRoute, settings: PresenterSettings) {
        if settings.low_battery_alert && settings.vibration_intensity > 0 {
            self.emit_haptic(PresenterHapticRequest {
                route,
                intensity: settings.vibration_intensity.min(100),
            });
        }
    }

    /// Whether the configured pointer action should first recenter the cursor.
    #[must_use]
    pub fn should_recenter(&self) -> bool {
        self.lock().configured.recenter_pointer
    }

    /// Whether the top pointer button is currently held.
    #[must_use]
    pub fn pointer_is_held(&self) -> bool {
        self.lock().pointer_held
    }

    /// Show the currently configured effect as an ordinary action.
    pub fn show_configured(&self) {
        let effect = self.lock().selected_effect;
        self.toggle(effect);
    }

    /// Handle the physical top-button press edge.
    ///
    /// Double-click cycling arrives separately as the firmware's
    /// `SWITCH_HIGHLIGHTING` task; keeping press and cycle separate prevents
    /// one physical double-click from advancing twice.
    pub fn pointer_pressed(&self) {
        let mut state = self.lock();
        // Freeze is a two-state interaction: the first release pins the
        // effect, and the next top-button press dismisses it. Without this
        // escape path a frozen overlay could only be cleared by quitting the
        // app or invoking a separately bound action.
        if state.frozen_position.is_some() && state.effect.is_some() {
            state.effect = None;
            state.frozen_position = None;
            state.pointer_held = false;
            bump_locked(&mut state);
            drop(state);
            self.changed.notify_waiters();
            return;
        }
        state.pointer_held = true;
        state.frozen_position = None;
        state.effect = Some(state.selected_effect);
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    /// Handle the top-button release edge.
    pub fn pointer_released(&self, cursor: Option<(f64, f64)>) {
        let mut state = self.lock();
        if !state.pointer_held {
            return;
        }
        state.pointer_held = false;
        if state.configured.freeze_effect {
            state.frozen_position = cursor.and_then(rounded_position);
        } else {
            state.effect = None;
            state.frozen_position = None;
        }
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    /// Advance to the next enabled effect.
    pub fn cycle_forward(&self) {
        self.cycle(CycleDirection::Forward);
    }

    /// Move to the previous enabled effect.
    pub fn cycle_backward(&self) {
        self.cycle(CycleDirection::Backward);
    }

    fn cycle(&self, direction: CycleDirection) {
        let mut state = self.lock();
        cycle_locked(&mut state, direction);
        state.frozen_position = None;
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    /// Start the timer on the first slide-navigation press. Later presses do
    /// not reset the configured presentation duration.
    pub fn start_configured_timer_once(&self) {
        if self.lock().timer_started {
            return;
        }
        self.start_configured_timer();
    }

    /// Start or restart the configured presentation timer.
    pub fn start_configured_timer(&self) {
        let state = self.lock();
        let effect = state.selected_effect;
        let seconds = state.configured.timer_seconds;
        let mode = state.configured.timer_mode;
        drop(state);
        match mode {
            PresenterTimerMode::Countdown if seconds > 0 => self.start_timer(effect, seconds),
            PresenterTimerMode::Off | PresenterTimerMode::Countdown => {}
            PresenterTimerMode::CurrentTime => self.start_current_time(effect),
        }
    }

    /// Start a countdown while showing `effect`.
    pub fn start_timer(&self, effect: PresenterEffect, seconds: u16) {
        if seconds == 0 {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(u64::from(seconds));
        self.set_timed_state(effect, Some(deadline), false);
        let manager = self.clone();
        tokio::spawn(async move {
            let mut warning_sent = false;
            loop {
                let Some(remaining) = manager.remaining() else {
                    return;
                };
                if remaining.is_zero() {
                    let request = manager.timer_haptic_request();
                    manager.stop_timer();
                    if let Some(request) = request {
                        manager.emit_haptic(request);
                    }
                    return;
                }
                if !warning_sent && remaining <= Duration::from_mins(5) && seconds > 5 * 60 {
                    if let Some(request) = manager.timer_haptic_request() {
                        manager.emit_haptic(request);
                    }
                    warning_sent = true;
                }
                tokio::time::sleep(remaining.min(Duration::from_millis(250))).await;
                manager.bump();
            }
        });
    }

    /// Show the configured effect with a live local wall-clock label.
    pub fn start_current_time(&self, effect: PresenterEffect) {
        self.set_timed_state(effect, None, true);
        let manager = self.clone();
        tokio::spawn(async move {
            loop {
                if !manager.current_time_active() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
                manager.bump();
            }
        });
    }

    /// Stop the current visual and timer.
    pub fn clear(&self) {
        let mut state = self.lock();
        state.effect = None;
        state.timer_deadline = None;
        state.timer_current_time = false;
        state.timer_started = false;
        state.frozen_position = None;
        state.pointer_held = false;
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    /// Wait for a state generation newer than `since`.
    pub async fn observe(&self, since: Generation) -> PresenterObservation {
        loop {
            let current = self.snapshot();
            if current.generation != since {
                return current;
            }
            let notified = self.changed.notified();
            if self.snapshot().generation != since {
                continue;
            }
            let _ = tokio::time::timeout(OBSERVE_HOLD, notified).await;
            if self.snapshot().generation == since {
                return self.snapshot();
            }
        }
    }

    fn snapshot(&self) -> PresenterObservation {
        let state = self.lock();
        let settings = state.configured;
        PresenterObservation {
            generation: state.generation,
            overlay: state.effect.map(|effect| PresenterOverlay {
                effect,
                timer_remaining_ms: state.timer_deadline.map(|deadline| {
                    u32::try_from(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(u128::from(u32::MAX)),
                    )
                    .unwrap_or(u32::MAX)
                }),
                timer_current_time: state.timer_current_time,
                effect_size: settings.effect_size,
                effect_contrast: settings.effect_contrast,
                effect_color: settings.effect_color,
                magnifier_color: settings.magnifier_color,
                frozen_position: state.frozen_position,
                cursor_control: settings.cursor_control,
                spotlight_radius: settings.spotlight_radius,
                magnifier_radius: settings.magnifier_radius,
                magnification: settings.normalized_magnification(),
            }),
        }
    }

    fn set_effect(&self, effect: Option<PresenterEffect>) {
        let mut state = self.lock();
        state.effect = effect;
        state.frozen_position = None;
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    fn set_timed_state(
        &self,
        effect: PresenterEffect,
        timer_deadline: Option<Instant>,
        timer_current_time: bool,
    ) {
        let mut state = self.lock();
        state.effect = Some(effect);
        state.timer_deadline = timer_deadline;
        state.timer_current_time = timer_current_time;
        state.timer_started = true;
        state.frozen_position = None;
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    fn stop_timer(&self) {
        let mut state = self.lock();
        state.effect = None;
        state.timer_deadline = None;
        state.timer_current_time = false;
        state.timer_started = false;
        state.frozen_position = None;
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
    }

    fn remaining(&self) -> Option<Duration> {
        self.lock()
            .timer_deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn bump(&self) {
        let mut state = self.lock();
        if !state.timer_started {
            return;
        }
        bump_locked(&mut state);
        drop(state);
        self.changed.notify_waiters();
        debug!("presenter timer tick");
    }

    fn current_time_active(&self) -> bool {
        let state = self.lock();
        state.timer_started && state.timer_current_time
    }

    fn timer_haptic_request(&self) -> Option<PresenterHapticRequest> {
        let state = self.lock();
        if !state.configured.haptic_alerts || state.configured.vibration_intensity == 0 {
            return None;
        }
        Some(PresenterHapticRequest {
            route: state.active_route.clone()?,
            intensity: state.configured.vibration_intensity,
        })
    }

    fn emit_haptic(&self, request: PresenterHapticRequest) {
        let sender = self
            .haptic_sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if sender.is_some_and(|sender| sender.send(request).is_err()) {
            debug!("presenter haptic worker unavailable");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RuntimeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone, Copy)]
enum CycleDirection {
    Forward,
    Backward,
}

fn bump_locked(state: &mut RuntimeState) {
    state.generation = state.generation.wrapping_add(1).max(1);
}

fn rounded_position((x, y): (f64, f64)) -> Option<(i32, i32)> {
    Some((rounded_coordinate(x)?, rounded_coordinate(y)?))
}

fn rounded_coordinate(value: f64) -> Option<i32> {
    let rounded = value.round();
    if !rounded.is_finite() || rounded < f64::from(i32::MIN) || rounded > f64::from(i32::MAX) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the explicit finite i32-range check above makes this conversion safe"
    )]
    Some(rounded as i32)
}

fn configured_effect(settings: &PresenterSettings) -> PresenterEffect {
    if settings.effect_enabled(settings.effect) {
        settings.effect
    } else {
        settings
            .normalized_effect_order()
            .into_iter()
            .find(|effect| settings.effect_enabled(*effect))
            .unwrap_or(PresenterEffect::Highlight)
    }
}

fn cycle_locked(state: &mut RuntimeState, direction: CycleDirection) {
    let settings = state.configured;
    let effects = settings.normalized_effect_order();
    let current = state.effect.unwrap_or(state.selected_effect);
    let effect_is_visible = state.effect.is_some();
    let start = effects
        .iter()
        .position(|effect| *effect == current)
        .unwrap_or(0);
    let next = (1..=effects.len())
        .map(|offset| match direction {
            CycleDirection::Forward => effects[(start + offset) % effects.len()],
            CycleDirection::Backward => {
                effects[(start + effects.len() - offset % effects.len()) % effects.len()]
            }
        })
        .find(|effect| settings.effect_enabled(*effect))
        .unwrap_or(current);
    state.selected_effect = next;
    state.effect = effect_is_visible.then_some(next);
}

#[cfg(test)]
mod tests {
    use super::{CycleDirection, PresenterManager, cycle_locked};
    use openlogi_core::hid::{PresenterEffect, PresenterSettings};
    use openlogi_hid::DeviceRoute;
    use tokio::sync::mpsc;

    fn active(manager: &PresenterManager) -> Option<PresenterEffect> {
        manager.snapshot().overlay.map(|overlay| overlay.effect)
    }

    #[test]
    fn press_is_momentary_by_default() {
        let manager = PresenterManager::default();
        manager.pointer_pressed();
        assert_eq!(active(&manager), Some(PresenterEffect::Highlight));
        manager.pointer_released(Some((10.0, 20.0)));
        assert_eq!(active(&manager), None);
    }

    #[test]
    fn freeze_is_explicit() {
        let manager = PresenterManager::default();
        let settings = PresenterSettings {
            freeze_effect: true,
            ..PresenterSettings::default()
        };
        manager.set_configured_settings(settings);
        manager.pointer_pressed();
        manager.pointer_released(Some((10.0, 20.0)));
        let overlay = manager.snapshot().overlay;
        assert_eq!(
            overlay.map(|value| value.frozen_position),
            Some(Some((10, 20)))
        );
    }

    #[test]
    fn pressing_pointer_again_dismisses_a_frozen_effect() {
        let manager = PresenterManager::default();
        manager.set_configured_settings(PresenterSettings {
            freeze_effect: true,
            ..PresenterSettings::default()
        });
        manager.pointer_pressed();
        manager.pointer_released(Some((10.0, 20.0)));
        assert!(active(&manager).is_some());

        manager.pointer_pressed();
        manager.pointer_released(Some((10.0, 20.0)));
        assert_eq!(active(&manager), None);
    }

    #[test]
    fn firmware_double_click_task_advances_after_the_first_release() {
        let manager = PresenterManager::default();
        manager.pointer_pressed();
        manager.pointer_released(None);
        manager.cycle_forward();

        assert_eq!(active(&manager), None);
        manager.pointer_pressed();
        assert_eq!(active(&manager), Some(PresenterEffect::Magnify));
    }

    #[test]
    fn cycled_effect_survives_the_next_config_refresh_and_press() {
        let manager = PresenterManager::default();
        let mut settings = PresenterSettings::default();
        manager.set_configured_settings(settings);
        manager.cycle_forward();
        assert_eq!(active(&manager), None);

        settings.magnifier_radius += 1;
        manager.set_configured_settings(settings);
        manager.pointer_pressed();

        assert_eq!(active(&manager), Some(PresenterEffect::Magnify));
    }

    #[test]
    fn explicit_default_effect_change_replaces_the_runtime_selection() {
        let manager = PresenterManager::default();
        manager.cycle_forward();
        manager.set_configured_settings(PresenterSettings {
            effect: PresenterEffect::DigitalLaser,
            ..PresenterSettings::default()
        });
        manager.pointer_pressed();

        assert_eq!(active(&manager), Some(PresenterEffect::DigitalLaser));
    }

    #[test]
    fn backward_cycle_skips_disabled_effects() {
        let manager = PresenterManager::default();
        let mut state = manager.lock();
        state.configured.enabled_effects =
            PresenterEffect::Highlight.bit() | PresenterEffect::DigitalLaser.bit();
        state.effect = Some(PresenterEffect::Highlight);
        cycle_locked(&mut state, CycleDirection::Backward);
        assert_eq!(state.effect, Some(PresenterEffect::DigitalLaser));
    }

    #[test]
    fn low_battery_alert_honors_toggle_and_intensity() {
        let manager = PresenterManager::default();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        manager.set_haptic_sender(sender);
        let route = DeviceRoute::Direct {
            vendor_id: 0x046d,
            product_id: 0xb503,
        };

        manager.alert_low_battery(
            route.clone(),
            PresenterSettings {
                low_battery_alert: false,
                ..PresenterSettings::default()
            },
        );
        let Err(_) = receiver.try_recv() else {
            panic!("disabled low-battery alert must stay silent");
        };

        manager.alert_low_battery(
            route.clone(),
            PresenterSettings {
                low_battery_alert: true,
                vibration_intensity: 73,
                ..PresenterSettings::default()
            },
        );
        let Ok(request) = receiver.try_recv() else {
            panic!("enabled low-battery alert must vibrate");
        };
        assert_eq!(request.route, route);
        assert_eq!(request.intensity, 73);
    }
}
