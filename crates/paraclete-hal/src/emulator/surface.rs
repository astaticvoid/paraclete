use paraclete_node_api::{Control, PadDescriptor, SurfaceDescriptor, PortName};

/// Build the Launchpad surface descriptor — 8×8 pad grid (ids 0–63, row-major),
/// 8 scene buttons (ids 64–71, right column), and 8 top control-row buttons
/// (ids 72–79).
///
/// Allocated once at `LaunchpadEmulator` construction and stored in the struct.
/// All pads are RGB; grid pads are velocity-sensitive.
pub(super) fn build_launchpad_surface() -> SurfaceDescriptor {
    let mut controls: Vec<Control> = Vec::with_capacity(64 + 8 + 8);

    // 8×8 grid — row-major: id = row * 8 + col
    for row in 0u8..8 {
        for col in 0u8..8 {
            let id = (row as u32) * 8 + (col as u32);
            controls.push(Control::Pad(PadDescriptor {
                id,
                name: PortName::Dynamic(format!("pad_{row}_{col}")),
                row: Some(row),
                col: Some(col),
                velocity_sensitive: true,
                pressure_sensitive: false,
                rgb: true,
            }));
        }
    }

    // 8 scene buttons on the right column (ids 64–71)
    for i in 0u32..8 {
        controls.push(Control::Pad(PadDescriptor {
            id: 64 + i,
            name: PortName::Dynamic(format!("scene_{i}")),
            row: Some(i as u8),
            col: None,
            velocity_sensitive: false,
            pressure_sensitive: false,
            rgb: true,
        }));
    }

    // 8 top control-row buttons (ids 72–79) — modes / navigation
    for i in 0u32..8 {
        controls.push(Control::Pad(PadDescriptor {
            id: 72 + i,
            name: PortName::Dynamic(format!("control_{i}")),
            row: None,
            col: Some(i as u8),
            velocity_sensitive: false,
            pressure_sensitive: false,
            rgb: true,
        }));
    }

    SurfaceDescriptor {
        name: "Launchpad Emulator",
        vendor: "Paraclete",
        controls,
    }
}
