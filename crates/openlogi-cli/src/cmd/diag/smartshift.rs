//! `openlogi diag smartshift` — SmartShift toggle round-trip.

use std::num::NonZeroU8;

use anyhow::{Context, Result};
use clap::Args;
use openlogi_hid::{
    SmartShiftAutoDisengage, TunableTorque, get_smartshift_status_on, set_smartshift_on,
    toggle_smartshift_on,
};

use crate::cmd::diag::select_device;

#[derive(Debug, Args)]
pub struct SmartshiftArgs {
    /// Leave the wheel in the toggled mode (skip the second toggle that
    /// restores the original). Useful for visually verifying the flip.
    #[arg(long, conflicts_with = "sensitivity")]
    pub leave_flipped: bool,

    /// Set the auto-disengage sensitivity instead of toggling, keeping the
    /// current Free/Ratchet mode. N is 1-255 (the wheel's speed threshold to
    /// free-spin): lower = more sensitive; typical 10-40; 255 = permanent
    /// ratchet (only meaningful while in Ratchet mode; the current mode is
    /// always preserved). `0` is rejected — the device treats it as "no change".
    #[arg(long, value_name = "N")]
    pub sensitivity: Option<NonZeroU8>,

    /// Run against the device whose name contains this string
    /// (case-insensitive) instead of auto-selecting. Useful when several
    /// devices are paired (e.g. a mouse and a keyboard over Bluetooth).
    #[arg(long, value_name = "NAME")]
    pub device: Option<String>,
}

pub async fn run(args: SmartshiftArgs) -> Result<()> {
    // 0x2110 / 0x2111 = SmartShift — auto-skip devices that expose neither.
    let (route, name, channel) = select_device(args.device.as_deref(), &[0x2110, 0x2111]).await?;
    println!("device: {name} ({route})");

    if let Some(n) = args.sensitivity {
        let requested = SmartShiftAutoDisengage::from(n);
        let before = get_smartshift_status_on(&channel)
            .await
            .context("read SmartShift status")?;
        println!(
            "  current: mode={:?} sensitivity={}",
            before.mode, before.auto_disengage
        );

        let mut desired = before;
        desired.auto_disengage = requested;
        set_smartshift_on(&channel, desired)
            .await
            .context("set SmartShift sensitivity")?;
        let after = get_smartshift_status_on(&channel)
            .await
            .context("read SmartShift after sensitivity write")?;
        println!(
            "  read-back: mode={:?} sensitivity={}",
            after.mode, after.auto_disengage
        );

        if after.auto_disengage != requested {
            anyhow::bail!(
                "SmartShift sensitivity write not applied: requested {n}, device reports {}",
                after.auto_disengage
            );
        }
        if after.mode != before.mode {
            anyhow::bail!(
                "SmartShift mode changed unexpectedly: was {:?}, now {:?}",
                before.mode,
                after.mode
            );
        }

        println!(
            "✓ SmartShift sensitivity set to {n} (mode {:?} preserved)",
            after.mode
        );
        return Ok(());
    }

    let before = get_smartshift_status_on(&channel)
        .await
        .context("read SmartShift status")?;
    println!(
        "  current: mode={:?} auto_disengage={} torque={}",
        before.mode,
        before.auto_disengage,
        before.tunable_torque.map_or(0, TunableTorque::into_inner)
    );

    let new_mode = toggle_smartshift_on(&channel)
        .await
        .context("toggle SmartShift")?;
    println!("  toggled to: {new_mode:?}");

    let after = get_smartshift_status_on(&channel)
        .await
        .context("read SmartShift after toggle")?;
    println!(
        "  read-back: mode={:?} auto_disengage={} torque={}",
        after.mode,
        after.auto_disengage,
        after.tunable_torque.map_or(0, TunableTorque::into_inner)
    );

    if after.mode == before.mode {
        anyhow::bail!(
            "SmartShift toggle had no effect: still {:?} after write",
            before.mode
        );
    }

    if args.leave_flipped {
        println!("✓ SmartShift toggle OK (wheel left in {new_mode:?})");
        return Ok(());
    }

    println!("  restoring mode: {:?}", before.mode);
    toggle_smartshift_on(&channel)
        .await
        .context("restore SmartShift")?;

    println!("✓ SmartShift round-trip OK");
    Ok(())
}
