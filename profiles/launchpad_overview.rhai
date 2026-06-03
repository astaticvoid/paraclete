// Step 6: pad press toggles a step on that row's sequencer.
fn on_load() {
    // Pad presses → CMD_TOGGLE_STEP on the matching track's sequencer.
    on_hw_event(LP_DEVICE_ID, |event| {
        if event.event_type == "PadPressed" {
            let id  = event.id;
            if id < 64 {
                let row = id / 8;
                let col = id % 8;
                send_cmd(TRACK_SEQ_IDS[row], 16, col, 0.0);
            }
        }
    });

    // Step display — same as step 5.
    for track in 0..8 {
        let t      = track;
        let seq_id = TRACK_SEQ_IDS[t];

        subscribe(`/node/${seq_id}/state/current_step`, |step| {
            let steps = get_step_state(seq_id);
            let col   = step % 8;
            for c in 0..8 {
                let pad    = t * 8 + c;
                let active = type_of(steps) == "string"
                             && c < steps.len()
                             && steps[c] == '1';
                if c == col {
                    set_led(LP_DEVICE_ID, pad, 0, 255, 0);
                } else if active {
                    set_led(LP_DEVICE_ID, pad, 0, 64, 255);
                } else {
                    set_led(LP_DEVICE_ID, pad, 0, 0, 0);
                }
            }
        });
    }
}
