use paraclete_clap::SingleNodePlugin;
use paraclete_node_api::{NodeCommand, TimedEvent, TransportEvent, TransportFlags, TransportInfo};
use paraclete_nodes::Sequencer;

// ── Helpers ───────────────────────────────────────────────────────────────────

fn playing_transport() -> TransportInfo {
    TransportInfo {
        playing: true,
        bpm: 120.0,
        ..TransportInfo::default()
    }
}

fn global_start_event() -> TimedEvent {
    let te = TransportEvent {
        domain_id: 0,
        bar: 1,
        beat: 0,
        tick: 0,
        ticks_per_beat: paraclete_node_api::TICKS_PER_BEAT,
        bpm: 120.0,
        time_sig_num: 4,
        time_sig_den: 4,
        flags: TransportFlags {
            global_start: true,
            playing: true,
            ..TransportFlags::default()
        },
    };
    TimedEvent::new(0, paraclete_node_api::Event::Transport(te))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Spec: single_node_plugin_init_activate_deactivate_no_panic
#[test]
fn single_node_plugin_init_activate_deactivate_no_panic() {
    let mut plugin = SingleNodePlugin::new(Box::new(Sequencer::new()));
    plugin.activate(44100.0, 512);
    plugin.deactivate();
    // If we reach here the lifecycle path compiles and does not crash.
}

/// Spec: single_node_plugin_process_passes_transport_to_node
///
/// Send a global_start TransportEvent to the plugin and verify no panic.
/// The Sequencer responds to global_start by entering playing state; detailed
/// sequencer output is covered by the runtime integration tests.
#[test]
fn single_node_plugin_process_passes_transport_to_node() {
    let mut plugin = SingleNodePlugin::new(Box::new(Sequencer::new()));
    plugin.activate(44100.0, 512);

    let transport = playing_transport();
    let events = [global_start_event()];
    let commands: [NodeCommand; 0] = [];

    // Must not panic.
    let _ = plugin.process_block(&transport, &events, &commands);
}

/// Spec: single_node_plugin_state_roundtrip
///
/// Configure a Sequencer with a non-default step pattern (step 3 active),
/// save state, load into a fresh plugin, and verify the restored bytes match.
#[test]
fn single_node_plugin_state_roundtrip() {
    // ── Build plugin1 with a modified step pattern ───────────────────────────
    let mut plugin1 = SingleNodePlugin::new(Box::new(Sequencer::new()));
    plugin1.activate(44100.0, 512);

    // Enable step 3 via CMD_TOGGLE_STEP
    let cmd = NodeCommand {
        target_id: 0,
        type_id: Sequencer::CMD_TOGGLE_STEP,
        arg0: 3,
        arg1: 0.0,
    };
    let transport = TransportInfo::default();
    plugin1.process_block(&transport, &[], &[cmd]);

    let saved = plugin1.state_save();

    // ── Restore into plugin2 ─────────────────────────────────────────────────
    let mut plugin2 = SingleNodePlugin::new(Box::new(Sequencer::new()));
    // state_load() before activate() — CLAP's standard session-restore flow.
    plugin2.state_load(&saved);
    plugin2.activate(44100.0, 512);

    let restored = plugin2.state_save();

    assert_eq!(
        saved, restored,
        "state_load() must restore the exact serialised bytes"
    );
}
