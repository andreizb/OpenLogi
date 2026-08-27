//! Logitech Spotlight pointer-speed scaling (`0x2205`).

use std::sync::Arc;

use hidpp::{
    device::Device,
    feature::{CreatableFeature, pointer_speed::PointerSpeedFeature},
    protocol::v20::{ErrorType, Hidpp20Error},
};
use openlogi_core::hid::PointerSpeed;

use crate::SharedChannel;
use crate::backend::HidBackend;
use crate::channel::route::DeviceRoute;

use super::{HidppOperation, WriteError, classify_hidpp_error, with_route};

const FEATURE_ID: u16 = PointerSpeedFeature::ID;

async fn open_pointer_speed(
    channel: &Arc<hidpp::channel::HidppChannel>,
    index: u8,
) -> Result<Arc<PointerSpeedFeature>, WriteError> {
    let mut device = Device::new(Arc::clone(channel), index)
        .await
        .map_err(|_| WriteError::DeviceUnreachable { index })?;
    let info = device
        .root()
        .get_feature(FEATURE_ID)
        .await
        .map_err(|error| classify_hidpp_error(error, HidppOperation::ResolveFeature, FEATURE_ID))?
        .ok_or(WriteError::FeatureUnsupported {
            feature_hex: FEATURE_ID,
        })?;
    Ok(device.add_feature(info.index))
}

fn decode_speed(value: hidpp::feature::pointer_speed::PointerSpeedLevel) -> PointerSpeed {
    // The HID++ wrapper has already rejected values outside 0..=9.
    PointerSpeed::new(value.get()).unwrap_or(PointerSpeed::MAX)
}

/// Read Spotlight's current pointer-speed level.
pub async fn get_pointer_speed(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
) -> Result<PointerSpeed, WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        get_pointer_speed_on_channel(&channel, index).await
    })
    .await
}

/// Read Spotlight's pointer speed on an already-open capture channel.
pub async fn get_pointer_speed_on(shared: &SharedChannel) -> Result<PointerSpeed, WriteError> {
    get_pointer_speed_on_channel(shared.channel(), shared.device_index()).await
}

async fn get_pointer_speed_on_channel(
    channel: &Arc<hidpp::channel::HidppChannel>,
    index: u8,
) -> Result<PointerSpeed, WriteError> {
    let feature = open_pointer_speed(channel, index).await?;
    feature
        .get_speed()
        .await
        .map(decode_speed)
        .map_err(|error| classify_pointer_speed_error(error, HidppOperation::ReadPointerSpeed))
}

/// Set Spotlight's pointer-speed level.
pub async fn set_pointer_speed(
    backend: &dyn HidBackend,
    route: &DeviceRoute,
    speed: PointerSpeed,
) -> Result<(), WriteError> {
    let index = route.device_index();
    with_route(backend, route, move |channel| async move {
        set_pointer_speed_on_channel(&channel, index, speed).await
    })
    .await
}

/// Set Spotlight's pointer speed on an already-open capture channel.
pub async fn set_pointer_speed_on(
    shared: &SharedChannel,
    speed: PointerSpeed,
) -> Result<(), WriteError> {
    set_pointer_speed_on_channel(shared.channel(), shared.device_index(), speed).await
}

async fn set_pointer_speed_on_channel(
    channel: &Arc<hidpp::channel::HidppChannel>,
    index: u8,
    speed: PointerSpeed,
) -> Result<(), WriteError> {
    let feature = open_pointer_speed(channel, index).await?;
    let wire = hidpp::feature::pointer_speed::PointerSpeedLevel::new(speed.get()).ok_or(
        WriteError::FeatureUnsupported {
            feature_hex: FEATURE_ID,
        },
    )?;
    feature
        .set_speed(wire)
        .await
        .map_err(|error| classify_pointer_speed_error(error, HidppOperation::WritePointerSpeed))
}

fn classify_pointer_speed_error(error: Hidpp20Error, operation: HidppOperation) -> WriteError {
    match error {
        Hidpp20Error::Feature(ErrorType::Unsupported | ErrorType::InvalidFunctionId)
        | Hidpp20Error::UnsupportedResponse => WriteError::FeatureUnsupported {
            feature_hex: FEATURE_ID,
        },
        other => classify_hidpp_error(other, operation, FEATURE_ID),
    }
}
