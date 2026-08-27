//! Wire-safe presenter settings shared by the agent, GUI, and HID layers.

use serde::{Deserialize, Serialize};

use crate::color::Rgb;

/// Host-rendered visual mode for a Logitech Spotlight presenter.
///
/// The firmware exposes the button and pointer transport, but the actual
/// laser/highlight/magnifier is a desktop feature. OpenLogi keeps that mode in
/// the device config so it survives restarts and can be changed independently
/// from the hardware pointer-speed register.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresenterEffect {
    /// A small coloured digital-laser dot follows the pointer.
    DigitalLaser,
    /// A circular clear area remains visible while the rest of the display is
    /// dimmed.
    #[default]
    Highlight,
    /// A circular lens shows a live, magnified composite of the desktop.
    Magnify,
}

impl PresenterEffect {
    /// Bit used by [`PresenterSettings::enabled_effects`].
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::DigitalLaser => 0b001,
            Self::Highlight => 0b010,
            Self::Magnify => 0b100,
        }
    }
}

/// Timer display mode offered by Logitech's presenter software.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PresenterTimerMode {
    /// Do not show a timer or alter the effect lifetime.
    #[default]
    Off,
    /// Count down from [`PresenterSettings::timer_seconds`].
    Countdown,
    /// Show the local wall-clock time while the presenter effect is active.
    CurrentTime,
}

/// Persisted host-side Spotlight settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent presenter preferences, not mutually exclusive state"
)]
pub struct PresenterSettings {
    /// Visual effect used by [`crate::binding::Action::PresenterPointer`].
    #[serde(default)]
    pub effect: PresenterEffect,
    /// Effects included in the physical top-button cycle. Options+ lets more
    /// than one effect be enabled; the default enables all three.
    #[serde(default = "default_enabled_effects")]
    pub enabled_effects: u8,
    /// Order used by double-click and pointer+Next/Back cycling.
    #[serde(default = "default_effect_order")]
    pub effect_order: [PresenterEffect; 3],
    /// Timer duration in seconds. Zero disables the timer.
    #[serde(default)]
    pub timer_seconds: u16,
    /// Whether the presenter timer is disabled, a countdown, or the current
    /// local time. Older configs with a non-zero duration and no field are
    /// treated as countdowns by [`Self::normalized_timer_mode`].
    #[serde(default)]
    pub timer_mode: PresenterTimerMode,
    /// Whether expiry should request a presenter haptic alert when supported.
    #[serde(default)]
    pub haptic_alerts: bool,
    /// Whether the device should vibrate when its battery becomes low.
    #[serde(default = "default_low_battery_alert")]
    pub low_battery_alert: bool,
    /// Firmware vibration strength as a user-facing percentage (`0..=100`).
    #[serde(default = "default_vibration_intensity")]
    pub vibration_intensity: u8,
    /// Recenter the pointer on the active display when the dedicated action is
    /// invoked.
    #[serde(default)]
    pub recenter_pointer: bool,
    /// Keep the host cursor visible at the centre of the effect so links and
    /// videos can be clicked during a presentation.
    #[serde(default = "default_cursor_control")]
    pub cursor_control: bool,
    /// Keep the pointer effect on screen after the top button is released.
    #[serde(default)]
    pub freeze_effect: bool,
    /// Relative effect size percentage (`50..=200`, default `100`).
    #[serde(default = "default_effect_size")]
    pub effect_size: u8,
    /// Relative effect contrast percentage (`25..=100`, default `100`).
    #[serde(default = "default_effect_contrast")]
    pub effect_contrast: u8,
    /// Spotlight clear-area radius in screen points (`40..=320`).
    #[serde(default = "default_spotlight_radius")]
    pub spotlight_radius: u16,
    /// Magnifier lens radius in screen points (`40..=260`).
    #[serde(default = "default_magnifier_radius")]
    pub magnifier_radius: u16,
    /// Magnification percentage (`100..=500`). Legacy `2..=5` config values
    /// are interpreted as `200..=500` by [`Self::normalized_magnification`].
    #[serde(default = "default_magnification")]
    pub magnification: u16,
    /// Digital-laser colour.
    #[serde(default = "default_effect_color")]
    pub effect_color: Rgb,
    /// Magnifier rim colour.
    #[serde(default = "default_magnifier_color")]
    pub magnifier_color: Rgb,
}

const fn default_effect_size() -> u8 {
    100
}

const fn default_enabled_effects() -> u8 {
    PresenterEffect::DigitalLaser.bit()
        | PresenterEffect::Highlight.bit()
        | PresenterEffect::Magnify.bit()
}

const fn default_effect_order() -> [PresenterEffect; 3] {
    [
        PresenterEffect::Highlight,
        PresenterEffect::Magnify,
        PresenterEffect::DigitalLaser,
    ]
}

const fn default_cursor_control() -> bool {
    true
}

const fn default_effect_contrast() -> u8 {
    100
}

const fn default_spotlight_radius() -> u16 {
    140
}

const fn default_magnifier_radius() -> u16 {
    110
}

const fn default_magnification() -> u16 {
    200
}

const fn default_effect_color() -> Rgb {
    Rgb::new(0xff, 0x3b, 0x30)
}

const fn default_magnifier_color() -> Rgb {
    Rgb::new(0xff, 0xff, 0xff)
}

const fn default_low_battery_alert() -> bool {
    true
}

const fn default_vibration_intensity() -> u8 {
    50
}

impl Default for PresenterSettings {
    fn default() -> Self {
        Self {
            effect: PresenterEffect::Highlight,
            enabled_effects: default_enabled_effects(),
            effect_order: default_effect_order(),
            timer_seconds: 0,
            timer_mode: PresenterTimerMode::Off,
            haptic_alerts: true,
            low_battery_alert: default_low_battery_alert(),
            vibration_intensity: default_vibration_intensity(),
            recenter_pointer: false,
            cursor_control: default_cursor_control(),
            freeze_effect: false,
            effect_size: default_effect_size(),
            effect_contrast: default_effect_contrast(),
            spotlight_radius: default_spotlight_radius(),
            magnifier_radius: default_magnifier_radius(),
            magnification: default_magnification(),
            effect_color: default_effect_color(),
            magnifier_color: default_magnifier_color(),
        }
    }
}

impl PresenterSettings {
    /// Return whether settings are the OpenLogi defaults and may be omitted
    /// from a device block in `config.toml`.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Whether an effect participates in the top-button double-click cycle.
    #[must_use]
    pub const fn effect_enabled(self, effect: PresenterEffect) -> bool {
        self.enabled_effects & effect.bit() != 0
    }

    /// Ensure a hand-edited mask always leaves at least one usable effect.
    #[must_use]
    pub const fn normalized_enabled_effects(self) -> u8 {
        let mask = self.enabled_effects & default_enabled_effects();
        if mask == 0 { self.effect.bit() } else { mask }
    }

    /// Resolve the legacy representation used before `timer_mode` existed.
    #[must_use]
    pub const fn normalized_timer_mode(self) -> PresenterTimerMode {
        match (self.timer_mode, self.timer_seconds) {
            (PresenterTimerMode::Off, seconds) if seconds > 0 => PresenterTimerMode::Countdown,
            (mode, _) => mode,
        }
    }

    /// Return a complete, duplicate-free effect order. Hand-edited malformed
    /// arrays fall back to the same order Logitech presents in its UI.
    #[must_use]
    pub fn normalized_effect_order(self) -> [PresenterEffect; 3] {
        let [first, second, third] = self.effect_order;
        if first == second || first == third || second == third {
            default_effect_order()
        } else {
            self.effect_order
        }
    }

    /// Resolve the legacy integer-factor representation and clamp a hand-edited
    /// percentage to the supported range.
    #[must_use]
    pub const fn normalized_magnification(self) -> u16 {
        match self.magnification {
            2..=5 => self.magnification * 100,
            0..=99 => 100,
            100..=500 => self.magnification,
            _ => 500,
        }
    }
}

/// Logitech Spotlight pointer-speed level.
///
/// Spotlight firmware exposes ten discrete values. The wrapper keeps invalid
/// values out of the public API while retaining the compact `u8` TOML/IPC
/// representation used by the rest of the HID settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PointerSpeed(u8);

impl PointerSpeed {
    /// Number of supported user-facing levels.
    pub const COUNT: u8 = 10;

    /// The slowest pointer-speed level.
    pub const MIN: Self = Self(0);

    /// The fastest pointer-speed level.
    pub const MAX: Self = Self(Self::COUNT - 1);

    /// Construct a valid Spotlight pointer-speed level.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the user-facing level (`0..=9`).
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl Default for PointerSpeed {
    fn default() -> Self {
        Self::MAX
    }
}

#[cfg(test)]
mod tests {
    use super::PointerSpeed;

    #[test]
    fn accepts_only_the_ten_firmware_levels() {
        assert_eq!(PointerSpeed::new(0), Some(PointerSpeed::MIN));
        assert_eq!(PointerSpeed::new(9), Some(PointerSpeed::MAX));
        assert_eq!(PointerSpeed::new(10), None);
    }
}
