//! Logitech Spotlight settings, bindings, and pointer-speed state.

use std::collections::BTreeMap;

use gpui::{App, Context};
use openlogi_core::binding::{Action, Binding, ButtonId};
use openlogi_core::bindings::presenter_bindings_for;
use openlogi_core::hid::{PointerSpeed, PresenterSettings};

use super::devices::DeviceRecord;
use super::{AppState, DeviceKey, Load, PointerSpeedLoad, StateEvent, StateEvents};

impl AppState {
    /// Effective presenter settings for the profile currently open in the UI.
    #[must_use]
    pub(crate) fn presenter_settings(&self) -> PresenterSettings {
        self.presenter_settings_for(self.editing_app())
    }

    /// Effective presenter settings for an explicit application profile.
    #[must_use]
    pub(crate) fn presenter_settings_for(&self, profile: Option<&str>) -> PresenterSettings {
        self.current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map_or_else(PresenterSettings::default, |key| {
                self.config.effective_presenter(key, profile)
            })
    }

    /// Effective Spotlight button actions for an explicit profile.
    #[must_use]
    pub(crate) fn presenter_bindings_for_profile(
        &self,
        profile: Option<&str>,
    ) -> BTreeMap<ButtonId, Action> {
        let key = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key);
        presenter_bindings_for(&self.config, key, profile)
            .into_iter()
            .map(|(button, binding)| (button, binding.click_action()))
            .collect()
    }

    /// Persist global or application-specific Spotlight settings. The agent
    /// reload resolves the currently frontmost application, so editing an
    /// inactive profile never leaks those settings into the active session.
    pub(crate) fn commit_presenter_settings_for(
        &mut self,
        profile: Option<&str>,
        settings: PresenterSettings,
    ) -> StateEvents {
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return StateEvents::none();
        };
        self.config.edit(|config| match profile {
            Some(app) => config.set_per_app_presenter(&key, app, Some(settings)),
            None => config.set_presenter(&key, settings),
        });
        self.persist_and_reload("presenter settings");
        self.for_current_device(StateEvent::PresenterChanged)
    }

    /// Commit one Spotlight button action into the selected profile.
    pub(crate) fn commit_presenter_binding_for(
        &mut self,
        profile: Option<&str>,
        button: ButtonId,
        action: Action,
    ) -> StateEvents {
        let Some(key) = self
            .current_record()
            .and_then(DeviceRecord::persistent_config_key)
            .map(str::to_string)
        else {
            return StateEvents::none();
        };
        self.config.edit(|config| match profile {
            Some(app) => config.set_per_app_binding(&key, app, button, Some(action)),
            None => config.set_binding(&key, button, Binding::Single(action)),
        });
        self.refresh_binding_projections();
        self.persist_and_reload("presenter binding");
        self.for_current_device(StateEvent::PresenterChanged)
    }

    pub(super) fn load_current_pointer_speed(&mut self, cx: &mut Context<Self>) {
        let Some((key, route)) = self
            .current_record()
            .and_then(|record| Some((record.device_key(), record.route.clone()?)))
        else {
            return;
        };
        self.pointer
            .reads
            .ensure_pointer_speed(key, route, self.ipc_sender(), cx);
    }

    /// Current pointer-speed load state for the selected device.
    #[must_use]
    pub(crate) fn pointer_speed_status(&self) -> PointerSpeedLoad {
        self.current_record().map_or(Load::Unknown, |record| {
            self.pointer
                .reads
                .pointer_speed_status(&record.device_key())
        })
    }

    /// Retry the selected presenter's pointer-speed read.
    pub(crate) fn retry_pointer_speed_read(cx: &mut App, key: DeviceKey) {
        Self::apply(cx, |state| {
            state.pointer.reads.retry_pointer_speed(&key);
            StateEvent::PresenterChanged(key).into()
        });
    }

    /// Commit a live pointer-speed level through the agent.
    pub(crate) fn commit_pointer_speed(&mut self, speed: PointerSpeed) -> StateEvents {
        let Some((key, route)) = self.current_record().and_then(|record| {
            record
                .route
                .clone()
                .map(|route| (record.device_key(), route))
        }) else {
            return StateEvents::none();
        };
        self.pointer.reads.set_pointer_speed_ready(&key, speed);
        self.send_ipc(crate::services::ipc::SetPointerSpeed { route, speed });
        StateEvent::PresenterChanged(key).into()
    }
}
