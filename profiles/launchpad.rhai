// Launchpad X — Digitakt-style profile  (ADR-020)
//
// SURFACE:
//   Row 0  [0..7]   [64 SEQ_EDIT]
//   Row 1  [8..15]  [65 SHIFT   ]
//   Rows 2–7: dark
//
// TRIGGER MODE (default, SEQ_EDIT unlit):
//   Pads 0–7  = tracks 0–7. Amber if track has steps, off if empty.
//   Flash white when sequencer fires that track.
//   Press pad = trigger sound immediately (CMD_TRIGGER).
//   SHIFT + pad 0–7 = select track → enter sequence mode.
//
// SEQUENCE MODE (SEQ_EDIT lit):
//   Pads 0–7  = steps 0–7  of selected track (row 0).
//   Pads 8–15 = steps 8–15 of selected track (row 1).
//   Blue = active, off = inactive.
//   Green = current position steps 0–7, cyan = steps 8–15.
//   Press pad = toggle step (CMD_TOGGLE_STEP).
//   SHIFT + pad 0–7 = select a different track.
//   SEQ_EDIT press = return to trigger mode.

fn on_load() {
    state_write("/script/lp/mode",     "trigger");
    state_write("/script/lp/selected", 0);
    state_write("/script/lp/shift",    false);

    // ── Button / pad input (Launchpad/emulator + Theoria: one handler) ───────
    on_surface_event([LP_DEVICE_ID, THEORIA_DEVICE_ID], |event| {
        let etype = event.event_type;
        let eid   = event.id;

        // SHIFT held / released
        if etype == "ButtonPressed"  && eid == 65 { state_write("/script/lp/shift", true);  return; }
        if etype == "ButtonReleased" && eid == 65 { state_write("/script/lp/shift", false); return; }

        // SEQ EDIT toggle
        if etype == "ButtonPressed" && eid == 64 {
            let mode = state_read("/script/lp/mode");
            debug_print(`SEQ_EDIT pressed: mode_before=${mode}`);
            if mode == "sequence" {
                // Back to trigger mode
                state_write("/script/lp/mode", "trigger");
                let check = state_read("/script/lp/mode");
                debug_print(`SEQ_EDIT: wrote trigger, readback=${check}`);
                set_led(LP_DEVICE_ID, 64, 0, 0, 0); // SEQ EDIT off
                for p in 8..16 { set_led(LP_DEVICE_ID, p, 0, 0, 0); }
                for t in 0..8 {
                    if t >= TRACK_SEQ_IDS.len() { set_led(LP_DEVICE_ID, t, 0, 0, 0); continue; }
                    let steps = get_step_state(TRACK_SEQ_IDS[t]);
                    let has   = type_of(steps) == "string" && steps.contains("1");
                    let r = if has { 48 } else { 0 };
                    let g = if has { 32 } else { 0 };
                    set_led(LP_DEVICE_ID, t, r, g, 0);
                }
            } else {
                // Enter sequence mode for selected track
                state_write("/script/lp/mode", "sequence");
                let check = state_read("/script/lp/mode");
                debug_print(`SEQ_EDIT: wrote sequence, readback=${check}`);
                set_led(LP_DEVICE_ID, 64, 0, 200, 100); // SEQ EDIT lit green
                // Full 16-step render
                let sel   = state_read("/script/lp/selected");
                let t     = if sel == () { 0 } else { sel };
                let sid   = TRACK_SEQ_IDS[t];
                let steps = get_step_state(sid);
                let cur   = state_read(`/node/${sid}/state/current_step`);
                let step  = if cur == () { 0 } else { cur };
                let col   = step % 8;
                let bank  = step / 8;
                for c in 0..8 {
                    let active = type_of(steps) == "string" && c < steps.len() && steps[c] == '1';
                    let s8     = c + 8;
                    let act8   = type_of(steps) == "string" && s8 < steps.len() && steps[s8] == '1';
                    let r0 = if c == col && bank == 0 { 0 } else if active { 0  } else { 0 };
                    let g0 = if c == col && bank == 0 { 255 } else if active { 64 } else { 0 };
                    let b0 = if c == col && bank == 0 { 0   } else if active { 255} else { 0 };
                    set_led(LP_DEVICE_ID, c,   r0, g0, b0);
                    let r1 = if c == col && bank == 1 { 0   } else if act8 { 0  } else { 0 };
                    let g1 = if c == col && bank == 1 { 255 } else if act8 { 64 } else { 0 };
                    let b1 = if c == col && bank == 1 { 180 } else if act8 { 255} else { 0 };
                    set_led(LP_DEVICE_ID, c+8, r1, g1, b1);
                }
            }
            return;
        }

        // Pad presses (rows 0–1 only)
        if etype != "PadPressed" || eid >= 16 { return; }

        let shift = state_read("/script/lp/shift");
        let mode  = state_read("/script/lp/mode");

        // SHIFT + pad 0–7 = select track (any mode → sequence mode)
        if shift == true && eid < 8 {
            if eid >= TRACK_SEQ_IDS.len() { return; } // fewer tracks than pads
            state_write("/script/lp/selected", eid);
            state_write("/script/lp/mode", "sequence");
            set_led(LP_DEVICE_ID, 64, 0, 200, 100);
            // Render new selection
            let sid   = TRACK_SEQ_IDS[eid];
            let steps = get_step_state(sid);
            let cur   = state_read(`/node/${sid}/state/current_step`);
            let step  = if cur == () { 0 } else { cur };
            let col   = step % 8;
            let bank  = step / 8;
            for c in 0..8 {
                let active = type_of(steps) == "string" && c < steps.len() && steps[c] == '1';
                let s8     = c + 8;
                let act8   = type_of(steps) == "string" && s8 < steps.len() && steps[s8] == '1';
                let r0 = if c == col && bank == 0 { 0 } else if active { 0  } else { 0 };
                let g0 = if c == col && bank == 0 { 255 } else if active { 64 } else { 0 };
                let b0 = if c == col && bank == 0 { 0   } else if active { 255} else { 0 };
                set_led(LP_DEVICE_ID, c,   r0, g0, b0);
                let r1 = if c == col && bank == 1 { 0   } else if act8 { 0  } else { 0 };
                let g1 = if c == col && bank == 1 { 255 } else if act8 { 64 } else { 0 };
                let b1 = if c == col && bank == 1 { 180 } else if act8 { 255} else { 0 };
                set_led(LP_DEVICE_ID, c+8, r1, g1, b1);
            }
            return;
        }

        debug_print(`pad eid=${eid} mode=${mode} shift=${shift}`);
        if mode == "sequence" {
            // Sequence mode: toggle step on selected track only.
            let sel = state_read("/script/lp/selected");
            let t   = if sel == () { 0 } else { sel };
            if t >= TRACK_SEQ_IDS.len() { return; }
            debug_print(`SEQ: CMD_TOGGLE_STEP track=${t} step=${eid}`);
            send_cmd(TRACK_SEQ_IDS[t], 16, eid, 0.0); // CMD_TOGGLE_STEP
        } else {
            // Trigger mode: fire sound on declared tracks.
            // CMD_TRIGGER (19) is implemented by AnalogEngine/FmEngine/Sampler
            // as of W1 C0; trigger mode makes sound (confirmed paired session #1).
            if eid < TRACK_GEN_IDS.len() {
                send_cmd(TRACK_GEN_IDS[eid], 19, 0, 0.0); // CMD_TRIGGER
                set_led(LP_DEVICE_ID, eid, 255, 255, 255);
            }
        }
    });

    // ── Generator play detection — logs which voice actually plays
    for track in 0..TRACK_GEN_IDS.len() {
        let t2      = track;
        let samp_id = TRACK_GEN_IDS[t2];
        subscribe(`/node/${samp_id}/state/trig`, |count| {
            let note = state_read(`/node/${samp_id}/state/last_note`);
            debug_print(`SAMPLER_PLAY track=${t2} samp=${samp_id} count=${count} note=${note}`);
        });
    }

    // ── Per-track subscriptions ───────────────────────────────────────────────
    for track in 0..TRACK_SEQ_IDS.len() {
        let t      = track;
        let seq_id = TRACK_SEQ_IDS[t];

        // Trig flash in trigger mode — logs which step actually fired
        subscribe(`/node/${seq_id}/state/last_trig`, |trig_count| {
            let fired_step = state_read(`/node/${seq_id}/state/last_fired_step`);
            debug_print(`TRIG track=${t} count=${trig_count} step=${fired_step}`);
            let mode = state_read("/script/lp/mode");
            if mode != "sequence" {
                set_led(LP_DEVICE_ID, t, 255, 255, 255);
            }
        });

        // Clock tick: move position indicator (sequence mode) or restore pad (trigger mode)
        subscribe(`/node/${seq_id}/state/current_step`, |step| {
            let mode = state_read("/script/lp/mode");
            let sel  = state_read("/script/lp/selected");
            let sel_t = if sel == () { 0 } else { sel };

            if mode == "sequence" && sel_t == t {
                // Full 16-step render with position indicator
                let steps = get_step_state(seq_id);
                let col   = step % 8;
                let bank  = step / 8;
                for c in 0..8 {
                    let active = type_of(steps) == "string" && c < steps.len() && steps[c] == '1';
                    let s8     = c + 8;
                    let act8   = type_of(steps) == "string" && s8 < steps.len() && steps[s8] == '1';
                    let r0 = if c == col && bank == 0 { 0 } else if active { 0  } else { 0 };
                    let g0 = if c == col && bank == 0 { 255 } else if active { 64 } else { 0 };
                    let b0 = if c == col && bank == 0 { 0   } else if active { 255} else { 0 };
                    set_led(LP_DEVICE_ID, c,   r0, g0, b0);
                    let r1 = if c == col && bank == 1 { 0   } else if act8 { 0  } else { 0 };
                    let g1 = if c == col && bank == 1 { 255 } else if act8 { 64 } else { 0 };
                    let b1 = if c == col && bank == 1 { 180 } else if act8 { 255} else { 0 };
                    set_led(LP_DEVICE_ID, c+8, r1, g1, b1);
                }
            } else if mode != "sequence" {
                // Restore trigger pad after flash
                let alive = state_alive(`/node/${seq_id}/state/last_trig`, 150);
                if !alive {
                    let steps = get_step_state(seq_id);
                    let has   = type_of(steps) == "string" && steps.contains("1");
                    let r = if has { 48 } else { 0 };
                    let g = if has { 32 } else { 0 };
                    set_led(LP_DEVICE_ID, t, r, g, 0);
                }
            }
        });

        // Steps changed: re-render sequence display or trigger pad
        subscribe(`/node/${seq_id}/state/steps`, |steps| {
            let mode  = state_read("/script/lp/mode");
            let sel   = state_read("/script/lp/selected");
            let sel_t = if sel == () { 0 } else { sel };

            if mode == "sequence" && sel_t == t {
                // Re-render 16 steps (position indicator comes from next current_step tick)
                for c in 0..8 {
                    let active = type_of(steps) == "string" && c < steps.len() && steps[c] == '1';
                    let r = if active { 0  } else { 0 };
                    let g = if active { 64 } else { 0 };
                    let b = if active { 255 } else { 0 };
                    set_led(LP_DEVICE_ID, c, r, g, b);
                    let s8   = c + 8;
                    let act8 = type_of(steps) == "string" && s8 < steps.len() && steps[s8] == '1';
                    let r8 = if act8 { 0  } else { 0 };
                    let g8 = if act8 { 64 } else { 0 };
                    let b8 = if act8 { 255 } else { 0 };
                    set_led(LP_DEVICE_ID, c + 8, r8, g8, b8);
                }
            } else if mode != "sequence" {
                let has = type_of(steps) == "string" && steps.contains("1");
                let r = if has { 48 } else { 0 };
                let g = if has { 32 } else { 0 };
                set_led(LP_DEVICE_ID, t, r, g, 0);
            }
        });
    }

    // ── Initial render: trigger mode ──────────────────────────────────────────
    for t in 0..8 {
        if t >= TRACK_SEQ_IDS.len() { set_led(LP_DEVICE_ID, t, 0, 0, 0); continue; }
        let steps = get_step_state(TRACK_SEQ_IDS[t]);
        let has   = type_of(steps) == "string" && steps.contains("1");
        let r = if has { 48 } else { 0 };
        let g = if has { 32 } else { 0 };
        set_led(LP_DEVICE_ID, t, r, g, 0);
    }
    for p in 8..16 { set_led(LP_DEVICE_ID, p, 0, 0, 0); }
    set_led(LP_DEVICE_ID, 64, 0, 0, 0);  // SEQ EDIT off
    set_led(LP_DEVICE_ID, 65, 8, 8, 0);  // SHIFT dim amber
}
