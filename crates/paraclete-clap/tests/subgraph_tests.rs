use paraclete_clap::SubgraphPlugin;

/// Node ID of the Sequencer in every SubgraphPlugin (matches subgraph::SEQ_ID).
const SEQ_ID: u32 = 2;
use paraclete_node_api::{
    capability::ParamDescriptor,
    midi::{u4, u7, ChannelVoice2, Channeled, Grouped, NoteOn},
    Event, NodeCommand, TimedEvent, TransportInfo, UmpMessage, CMD_SET_PARAM,
};
use paraclete_nodes::{AnalogEngine, FmEngine, Sequencer};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn playing_transport() -> TransportInfo {
    TransportInfo {
        playing: true,
        bpm: 120.0,
        ..TransportInfo::default()
    }
}

fn stopped_transport() -> TransportInfo {
    TransportInfo::default()
}

fn make_note_on(note: u8, velocity: u16) -> TimedEvent {
    let mut msg = NoteOn::<[u32; 4]>::new();
    msg.set_group(u4::new(0));
    msg.set_channel(u4::new(0));
    msg.set_note_number(u7::new(note & 0x7F));
    msg.set_velocity(velocity);
    let ump = UmpMessage::from(ChannelVoice2::from(msg));
    TimedEvent::new(0, Event::Midi2(ump))
}

fn kick_plugin() -> SubgraphPlugin {
    SubgraphPlugin::new(Box::new(AnalogEngine::kick()), 3, 44100.0, 512)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Spec: subgraph_plugin_init_activate_deactivate_no_panic
#[test]
fn subgraph_plugin_init_activate_deactivate_no_panic() {
    let mut plugin = kick_plugin();
    plugin.activate(44100.0, 512);
    plugin.deactivate();
    // Reaching here without panic confirms the lifecycle path works.
}

/// Spec: subgraph_plugin_direct_note_on_produces_audio
///
/// External NoteOn events are routed directly to the generator (bypassing the
/// internal Sequencer). After a few blocks the generator should produce
/// non-silent output.
#[test]
fn subgraph_plugin_direct_note_on_produces_audio() {
    let mut plugin = kick_plugin();
    plugin.activate(44100.0, 512);

    let note_on = make_note_on(60, 32768);
    let transport = playing_transport();

    // Block 1: inject note-on directly to the generator.
    let _ = plugin.process_block(&transport, None, &[note_on], &[]);
    // Blocks 2–3: let the synthesis run.
    let _ = plugin.process_block(&transport, None, &[], &[]);
    let out = plugin.process_block(&transport, None, &[], &[]);

    let max_sample = out.iter().map(|s| s.abs()).fold(0.0_f32, f32::max);
    assert!(
        max_sample > 1e-5,
        "expected non-silent audio after 3 blocks (max={max_sample})"
    );
}

/// Spec: subgraph_plugin_state_roundtrip
///
/// Save state with a Sequencer step enabled, restore into a fresh plugin,
/// save again — the bytes must match.
#[test]
fn subgraph_plugin_state_roundtrip() {
    // ── Build plugin1 with step 1 active in the Sequencer ────────────────────
    let mut plugin1 = kick_plugin();
    plugin1.activate(44100.0, 512);

    plugin1.send_command(NodeCommand {
        target_id: SEQ_ID,
        type_id: Sequencer::CMD_TOGGLE_STEP,
        arg0: 1,
        arg1: 0.0,
    });
    // Run one block so the command is processed (injected into pending_cmds).
    plugin1.process_block(&stopped_transport(), None, &[], &[]);

    let saved = plugin1.state_save();

    // The toggled step must be captured in the blob (not just a constant).
    let default_bytes = kick_plugin().state_save();
    assert_ne!(
        saved, default_bytes,
        "toggled step must change the serialised bytes"
    );

    // ── Restore into plugin2 ─────────────────────────────────────────────────
    let mut plugin2 = kick_plugin();
    plugin2.state_load(&saved);
    plugin2.activate(44100.0, 512);

    let restored = plugin2.state_save();

    assert_eq!(
        saved, restored,
        "state_load must restore the exact serialised bytes"
    );
}

/// Spec: subgraph_plugin_seq_command_reaches_sequencer
///
/// CMD_TOGGLE_STEP sent to the Sequencer via `send_command` must flip the
/// step's active flag. Verified by comparing state_save bytes before/after.
#[test]
fn subgraph_plugin_seq_command_reaches_sequencer() {
    let mut plugin = kick_plugin();
    plugin.activate(44100.0, 512);

    let bytes_before = plugin.state_save();

    plugin.send_command(NodeCommand {
        target_id: SEQ_ID,
        type_id: Sequencer::CMD_TOGGLE_STEP,
        arg0: 4,
        arg1: 0.0,
    });
    plugin.process_block(&stopped_transport(), None, &[], &[]);

    let bytes_after = plugin.state_save();

    assert_ne!(
        bytes_before, bytes_after,
        "toggling a step must change the serialised Sequencer state"
    );
}

/// Spec: subgraph_plugin_gen_command_reaches_generator
///
/// A CMD_SET_PARAM command passed via `process_block` must reach the generator
/// without panicking. The test uses the "decay" parameter present on AnalogEngine.
#[test]
fn subgraph_plugin_gen_command_reaches_generator() {
    let mut plugin = kick_plugin();
    plugin.activate(44100.0, 512);

    let decay_id = ParamDescriptor::id_for_name("decay");
    let cmd = NodeCommand {
        target_id: 0, // target_id overridden by process_block to gen_id
        type_id: CMD_SET_PARAM,
        arg0: decay_id as i64,
        arg1: 0.3,
    };

    // Must not panic.
    let _ = plugin.process_block(&stopped_transport(), None, &[], &[cmd]);
}

/// Spec: subgraph_plugin_fm_engine_variant_no_panic
///
/// SubgraphPlugin must construct and process without panic when wrapping
/// FmEngine::bass().
#[test]
fn subgraph_plugin_fm_engine_variant_no_panic() {
    let mut plugin = SubgraphPlugin::new(Box::new(FmEngine::bass()), 3, 44100.0, 512);
    plugin.activate(44100.0, 512);

    let note_on = make_note_on(48, 32768);
    let transport = playing_transport();

    // Must not panic.
    let _ = plugin.process_block(&transport, None, &[note_on], &[]);
    let _ = plugin.process_block(&transport, None, &[], &[]);
}
