// Digitakt MIDI profile — contextual parameter control.
//
// 8 relative encoders control parameters on the selected track.
// Encoder 0: distortion drive  (PARAM_DRIVE = 0)
// Encoder 1: filter cutoff     (PARAM_CUTOFF = 0)
// Encoder 2: filter resonance  (PARAM_RESONANCE = 1)
// Encoder 3: distortion blend  (PARAM_BLEND = 2)
// Track buttons 8–15: select active track
//
// NOTE: Sampler pitch param_id is FNV-1a("pitch"), not a sequential int.
// Sampler does not yet use ParameterBank. Encoder → pitch deferred to P5.

fn on_load() {
    on_hw_event(DT_DEVICE_ID, |event| {
        if event.event_type == "EncoderChanged" {
            let track = state_read("/script/selected_track");
            let t = if track == () { 0 } else { track };
            route_encoder(event.id, event.delta, t);
        }
        if event.event_type == "ButtonPressed" {
            let id = event.id;
            if id >= 8 && id <= 15 {
                let track = id - 8;
                state_write("/script/selected_track", track);
            }
        }
    });
}

fn route_encoder(enc_id, delta, track) {
    let normalized = delta / 64.0;

    // DistortionNode: PARAM_DRIVE=0, PARAM_OUTPUT_LEVEL=1, PARAM_BLEND=2
    // FilterNode:     PARAM_CUTOFF=0, PARAM_RESONANCE=1, PARAM_FILTER_TYPE=2
    // type_id=1 = CMD_BUMP_PARAM
    if      enc_id == 0 { send_cmd(TRACK_DIST_IDS[track], 1, 0, normalized); }
    else if enc_id == 1 { send_cmd(TRACK_FILT_IDS[track], 1, 0, normalized * 4000.0); }
    else if enc_id == 2 { send_cmd(TRACK_FILT_IDS[track], 1, 1, normalized * 0.5); }
    else if enc_id == 3 { send_cmd(TRACK_DIST_IDS[track], 1, 2, normalized); }
}
