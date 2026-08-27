//! Implements Logitech's original Spotlight presenter-control feature
//! (`0x1a00`).
//!
//! The public HID++ feature catalogue does not describe this feature in
//! enough detail for a generic implementation. Its vibration command is
//! nevertheless stable across the original Spotlight firmware family and is
//! documented by the Projecteur interoperability notes: function `1` takes a
//! duration (`0..=10`), the fixed marker `0xe8`, and an 8-bit intensity.

use openlogi_hidpp_derive::Feature;

use crate::{feature::FeatureEndpoint, protocol::v20::Hidpp20Error};

/// Duration of a Spotlight vibration pulse, in the firmware's discrete units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenterVibrationLength(u8);

impl PresenterVibrationLength {
    /// Maximum duration accepted by the original Spotlight firmware.
    pub const MAX: u8 = 10;

    /// Validate and construct a pulse duration.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the wire value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Raw Spotlight vibration intensity (`0..=255`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PresenterVibrationIntensity(u8);

impl PresenterVibrationIntensity {
    /// Construct an intensity from its wire value.
    #[must_use]
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Return the wire value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Implements `PresenterControl` / `0x1a00`.
#[derive(Clone, Feature)]
#[creatable(id = 0x1a00, version = 0)]
pub struct PresenterControlFeature {
    /// The endpoint this feature talks to.
    endpoint: FeatureEndpoint,
}

impl PresenterControlFeature {
    /// Trigger one vibration pulse on an original Spotlight presenter.
    pub async fn vibrate(
        &self,
        length: PresenterVibrationLength,
        intensity: PresenterVibrationIntensity,
    ) -> Result<(), Hidpp20Error> {
        self.endpoint
            .call(1, [length.get(), 0xe8, intensity.get()])
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PresenterVibrationLength;

    #[test]
    fn length_is_limited_to_the_firmware_range() {
        assert_eq!(
            PresenterVibrationLength::new(0).map(PresenterVibrationLength::get),
            Some(0)
        );
        assert_eq!(
            PresenterVibrationLength::new(10).map(PresenterVibrationLength::get),
            Some(10)
        );
        assert_eq!(PresenterVibrationLength::new(11), None);
    }
}
