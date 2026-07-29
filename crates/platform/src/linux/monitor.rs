use crate::MonitorInfo;
use inputsync_core::Result;

/// X11/Wayland monitor enumeration stub.
pub fn enumerate() -> Result<Vec<MonitorInfo>> {
    // TODO: wire to `x11rb` XRandR query on X11; `wl_output` on Wayland.
    Ok(vec![])
}
