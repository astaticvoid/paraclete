use paraclete_node_api::capability::ParamDescriptor;
use paraclete_clap_host::{HostParamBridge, scan_clap_paths};

#[test]
fn host_param_bridge_from_raw_params_round_trips() {
    let decay_clap_id:  u32 = 101;
    let cutoff_clap_id: u32 = 202;

    let bridge = HostParamBridge::from_raw_params(&[
        (decay_clap_id,  "decay",  0.01, 4.0,     0.2),
        (cutoff_clap_id, "cutoff", 20.0, 20000.0, 1000.0),
    ]);

    let decay_paraclete_id  = ParamDescriptor::id_for_name("decay");
    let cutoff_paraclete_id = ParamDescriptor::id_for_name("cutoff");

    assert_eq!(bridge.paraclete_id_for(decay_clap_id),  Some(decay_paraclete_id));
    assert_eq!(bridge.paraclete_id_for(cutoff_clap_id), Some(cutoff_paraclete_id));
    assert_eq!(bridge.clap_id_for(decay_paraclete_id),  Some(decay_clap_id));
    assert_eq!(bridge.clap_id_for(cutoff_paraclete_id), Some(cutoff_clap_id));
}

#[test]
fn host_param_bridge_to_capability_document_has_correct_ranges() {
    let bridge = HostParamBridge::from_raw_params(&[
        (1, "decay",  0.01, 4.0,     0.2),
        (2, "cutoff", 20.0, 20000.0, 1000.0),
    ]);

    let doc = bridge.to_capability_document("TestPlugin", "TestVendor");
    assert_eq!(doc.params.len(), 2);

    let decay_id = ParamDescriptor::id_for_name("decay");
    let decay = doc.params.iter().find(|p| p.id == decay_id).expect("decay param");
    assert!((decay.min - 0.01).abs() < 1e-9);
    assert!((decay.max - 4.0).abs()  < 1e-9);

    let cutoff_id = ParamDescriptor::id_for_name("cutoff");
    let cutoff = doc.params.iter().find(|p| p.id == cutoff_id).expect("cutoff param");
    assert!((cutoff.min - 20.0).abs()    < 1e-9);
    assert!((cutoff.max - 20000.0).abs() < 1e-9);
}

#[test]
fn scan_clap_paths_returns_vec_no_panic() {
    // Must not panic even if no CLAP directories exist.
    let paths = scan_clap_paths();
    // All returned paths must have the .clap extension.
    for p in &paths {
        assert_eq!(
            p.extension().and_then(|e| e.to_str()),
            Some("clap"),
            "unexpected extension in {:?}", p
        );
    }
}

// ── Integration tests — require build artifacts (run with: cargo test -- --ignored) ──

fn dylib_path(stem: &str) -> std::path::PathBuf {
    let prefix = if cfg!(target_os = "windows") { "" } else { "lib" };
    let ext = if cfg!(target_os = "macos")   { "dylib" }
              else if cfg!(target_os = "linux") { "so" }
              else { "dll" };
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/debug")
        .join(format!("{prefix}{stem}.{ext}"))
}

/// Requires: `cargo build -p paraclete-machine-kick`
/// Run: `cargo test -p paraclete-clap-host -- --ignored`
#[test]
#[ignore]
fn plugin_node_from_kick_clap_produces_audio() {
    use paraclete_clap_host::PluginLibrary;
    use paraclete_node_api::{
        Event, ExtendedEventSlab, TimedEvent, UmpMessage,
        context::{EventOutputBuffer, ProcessOutput},
        buffer::AudioBuffer,
        transport::TransportInfo,
        midi::{ChannelVoice2, Channeled, Grouped, NoteOn, u4, u7},
        context::ProcessInput,
    };

    let lib_path = dylib_path("kick");
    let lib = PluginLibrary::load(&lib_path).expect("load kick library");
    assert!(!lib.descriptors().is_empty());

    let plugin_id = lib.descriptors()[0].id.clone();
    let mut node = lib.instantiate(&plugin_id, 48000.0, 512).expect("instantiate");
    node.activate(48000.0, 512);

    // Build a NoteOn event.
    let mut msg = NoteOn::<[u32; 4]>::new();
    msg.set_group(u4::new(0));
    msg.set_channel(u4::new(0));
    msg.set_note_number(u7::new(60));
    msg.set_velocity(32768u16);
    let ump = UmpMessage::from(ChannelVoice2::from(msg));
    let note_on = TimedEvent::new(0, Event::Midi2(ump));

    let transport = TransportInfo::default();
    let extended  = ExtendedEventSlab::empty();
    let commands: Vec<paraclete_node_api::NodeCommand> = vec![];
    let audio_ins: Vec<&AudioBuffer> = vec![];
    let sig_ins = [];

    let mut max_val: f32 = 0.0;
    let events = vec![note_on];
    for block in 0..4 {
        let block_events: &[TimedEvent] = if block == 0 { &events } else { &[] };
        let mut out_audio = AudioBuffer::new(1, 512);
        let mut out_events = EventOutputBuffer::new(32);
        let mut audio_outs = [&mut out_audio];
        let mut sig_outs = [];

        let input = ProcessInput {
            audio_inputs:    &audio_ins,
            signal_inputs:   &sig_ins,
            events:          block_events,
            transport:       &transport,
            sample_rate:     48000.0,
            block_size:      512,
            extended_events: &extended,
            commands:        &commands,
        };
        let mut output = ProcessOutput::new(
            &mut audio_outs,
            &mut sig_outs,
            &mut out_events,
        );
        node.process(&input, &mut output);
        for &s in out_audio.channel(0) {
            max_val = max_val.max(s.abs());
        }
    }
    assert!(max_val > 1e-5, "expected non-silent audio, got max={max_val}");
}

#[test]
#[ignore]
fn plugin_node_capability_document_has_params() {
    use paraclete_clap_host::PluginLibrary;


    let lib_path = dylib_path("kick");
    let lib = PluginLibrary::load(&lib_path).expect("load kick library");
    let plugin_id = lib.descriptors()[0].id.clone();
    let node = lib.instantiate(&plugin_id, 48000.0, 512).expect("instantiate");

    let doc = node.capability_document();
    assert!(!doc.params.is_empty(), "kick should expose at least one parameter");

    let decay_id = ParamDescriptor::id_for_name("decay");
    assert!(
        doc.params.iter().any(|p| p.id == decay_id),
        "kick should expose 'decay' parameter"
    );
}

#[test]
#[ignore]
fn plugin_node_serialize_deserialize_roundtrip() {
    use paraclete_clap_host::PluginLibrary;


    let lib_path = dylib_path("kick");
    let lib = PluginLibrary::load(&lib_path).expect("load kick library");
    let plugin_id = lib.descriptors()[0].id.clone();
    let mut node = lib.instantiate(&plugin_id, 48000.0, 512).expect("instantiate");
    node.activate(48000.0, 512);

    let data = node.serialize();
    // Must not panic even on empty state data.
    node.deserialize(&data);
}
