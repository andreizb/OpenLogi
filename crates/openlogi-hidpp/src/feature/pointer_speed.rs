//! Implements Logitech's Spotlight pointer-motion scaling feature (`0x2205`).
//!
//! The feature reports a ten-step hardware pointer scale. The wire values are
//! `0x10..=0x19`, while the user-facing level is `0..=9`; this encoding is
//! documented by the original Spotlight interoperability notes.

use openlogi_hidpp_derive::Feature;

use crate::{feature::FeatureEndpoint, protocol::v20::Hidpp20Error};

/// A Spotlight pointer-speed level (`0` is slowest, `9` is fastest).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PointerSpeedLevel(u8);

impl PointerSpeedLevel {
    /// Number of user-facing levels.
    pub const COUNT: u8 = 10;

    /// Construct a level in the supported `0..=9` range.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the user-facing level.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Convert to the feature's wire encoding.
    #[must_use]
    pub const fn wire(self) -> u8 {
        0x10 + self.0
    }

    /// Decode a feature wire value.
    #[must_use]
    pub const fn from_wire(value: u8) -> Option<Self> {
        if value >= 0x10 && value <= 0x19 {
            Some(Self(value - 0x10))
        } else {
            None
        }
    }
}

/// Implements `PointerMotionScaling` / `0x2205`.
#[derive(Clone, Feature)]
#[creatable(id = 0x2205, version = 0)]
pub struct PointerSpeedFeature {
    /// The endpoint this feature talks to.
    endpoint: FeatureEndpoint,
}

impl PointerSpeedFeature {
    /// Read the current pointer-speed level.
    pub async fn get_speed(&self) -> Result<PointerSpeedLevel, Hidpp20Error> {
        let payload = self.endpoint.call(0, [0; 3]).await?.extend_payload();
        PointerSpeedLevel::from_wire(payload[0]).ok_or(Hidpp20Error::UnsupportedResponse)
    }

    /// Set the current pointer-speed level.
    pub async fn set_speed(&self, speed: PointerSpeedLevel) -> Result<(), Hidpp20Error> {
        self.endpoint.call(1, [speed.wire(), 0, 0]).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::PointerSpeedLevel;

    #[test]
    fn encodes_and_decodes_the_ten_wire_levels() {
        for level in 0..10 {
            let speed = PointerSpeedLevel::new(level).expect("valid level");
            assert_eq!(PointerSpeedLevel::from_wire(speed.wire()), Some(speed));
        }
        assert_eq!(PointerSpeedLevel::new(10), None);
        assert_eq!(PointerSpeedLevel::from_wire(0x0f), None);
        assert_eq!(PointerSpeedLevel::from_wire(0x1a), None);
    }
}
