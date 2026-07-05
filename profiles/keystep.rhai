// Keystep 37 profile.
//
// Note On/Off events are routed through the graph directly:
//   KeystepNode → SurfaceMappingNode → Sequencer[7] (bass track)
//
// This profile handles the mod wheel → filter cutoff on the bass track.

fn on_load() {
    if KS_DEVICE_ID == 0 { return; }

    on_surface_event(KS_DEVICE_ID, |event| {
        if event.event_type == "FaderMoved" && event.id == 1 {
            // Mod wheel: scale 0–65535 → 200–4200 Hz cutoff
            let cutoff = 200.0 + (event.value / 65535.0) * 4000.0;
            send_cmd(TRACK_FILT_IDS[7], 0, 0, cutoff); // CMD_SET_PARAM, cutoff_hz
        }
    });
}
