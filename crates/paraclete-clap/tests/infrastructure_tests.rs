use paraclete_clap::ffi;
use paraclete_clap::bridge::ClapParamBridge;
use paraclete_clap::transport::{
    translate_transport, CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_HAS_BEATS_TIMELINE,
};
use paraclete_node_api::{CapabilityDocument, ParamDescriptor, ParamUnit, PortName, CMD_SET_PARAM};

fn make_three_param_doc() -> CapabilityDocument {
    CapabilityDocument {
        name:    "Test".into(),
        vendor:  "Test".into(),
        version: (0, 1, 0),
        ports:   vec![],
        params:  vec![
            ParamDescriptor {
                id:      ParamDescriptor::id_for_name("cutoff"),
                name:    PortName::Static("cutoff"),
                min:     0.0, max: 1000.0, default: 0.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id:      ParamDescriptor::id_for_name("resonance"),
                name:    PortName::Static("resonance"),
                min:     0.0, max: 1.0, default: 0.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id:      ParamDescriptor::id_for_name("drive"),
                name:    PortName::Static("drive"),
                min:     0.0, max: 1.0, default: 0.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            },
        ],
        extensions: vec![],
    }
}

fn make_two_param_doc() -> CapabilityDocument {
    CapabilityDocument {
        name:    "Test".into(),
        vendor:  "Test".into(),
        version: (0, 1, 0),
        ports:   vec![],
        params:  vec![
            ParamDescriptor {
                id:      ParamDescriptor::id_for_name("cutoff"),
                name:    PortName::Static("cutoff"),
                min:     0.0, max: 1000.0, default: 0.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            },
            ParamDescriptor {
                id:      ParamDescriptor::id_for_name("resonance"),
                name:    PortName::Static("resonance"),
                min:     0.0, max: 1.0, default: 0.0,
                stepped: false, unit: ParamUnit::Generic, display: None,
            },
        ],
        extensions: vec![],
    }
}

#[test]
fn bridge_from_cap_doc_assigns_sequential_ids() {
    let doc = make_three_param_doc();
    let bridge = ClapParamBridge::from_capability_document(&doc);

    assert_eq!(bridge.len(), 3);
    assert_eq!(
        bridge.paraclete_id_for(0),
        Some(ParamDescriptor::id_for_name("cutoff"))
    );
    assert_eq!(
        bridge.paraclete_id_for(1),
        Some(ParamDescriptor::id_for_name("resonance"))
    );
    assert_eq!(
        bridge.paraclete_id_for(2),
        Some(ParamDescriptor::id_for_name("drive"))
    );
}

#[test]
fn bridge_unknown_clap_id_returns_none() {
    let doc = make_two_param_doc();
    let bridge = ClapParamBridge::from_capability_document(&doc);
    assert_eq!(bridge.paraclete_id_for(99), None);
}

#[test]
fn bridge_make_set_param_command_correct() {
    let doc = make_two_param_doc();
    let bridge = ClapParamBridge::from_capability_document(&doc);

    let cmd = bridge.make_set_param_command(0, 1200.0, 42)
        .expect("clap_id=0 should exist");

    assert_eq!(cmd.type_id,  CMD_SET_PARAM);
    assert_eq!(cmd.target_id, 42);
    assert_eq!(cmd.arg0, ParamDescriptor::id_for_name("cutoff") as i64);
    assert_eq!(cmd.arg1, 1200.0);
}

#[test]
fn translate_transport_start_transition_emits_global_start() {
    // prev_playing=false → playing=true: global_start event expected.
    let flags = CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
    let (info, event) = translate_transport(flags, 140.0, 0, false);

    assert!(info.playing, "info.playing should be true when IS_PLAYING flag is set");
    assert_eq!(info.bpm, 140.0);

    let ev = event.expect("event should be Some on play-start transition");
    assert!(ev.flags.global_start, "global_start should be true on play-start transition");
    assert!(!ev.flags.global_stop, "global_stop should be false on play-start");
}

#[test]
fn translate_transport_no_event_when_already_playing() {
    // prev_playing=true, still playing: no transition event.
    let flags = CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
    let (info, event) = translate_transport(flags, 140.0, 0, true);

    assert!(info.playing);
    assert!(event.is_none(), "no event when playing state unchanged");
}

#[test]
fn translate_transport_stop_transition_emits_global_stop() {
    // prev_playing=true → playing=false: global_stop event expected.
    let (info, event) = translate_transport(0, 120.0, 0, true);

    assert!(!info.playing, "info.playing should be false when IS_PLAYING is not set");

    let ev = event.expect("event should be Some on stop transition");
    assert!(ev.flags.global_stop,  "global_stop should be true on stop transition");
    assert!(!ev.flags.global_start, "global_start should be false on stop");
}

#[test]
fn translate_transport_no_event_when_already_stopped() {
    // prev_playing=false, still stopped: no event.
    let (info, event) = translate_transport(0, 120.0, 0, false);

    assert!(!info.playing);
    assert!(event.is_none(), "no event when stopped state unchanged");
}

#[test]
fn machine_bank_ffi_entry_not_null() {
    let vtable = ffi::plugin_class();
    assert!(vtable.init.is_some(),             "plugin.init must be non-null");
    assert!(vtable.destroy.is_some(),          "plugin.destroy must be non-null");
    assert!(vtable.activate.is_some(),         "plugin.activate must be non-null");
    assert!(vtable.deactivate.is_some(),       "plugin.deactivate must be non-null");
    assert!(vtable.start_processing.is_some(), "plugin.start_processing must be non-null");
    assert!(vtable.stop_processing.is_some(),  "plugin.stop_processing must be non-null");
    assert!(vtable.reset.is_some(),            "plugin.reset must be non-null");
    assert!(vtable.process.is_some(),          "plugin.process must be non-null");
    assert!(vtable.get_extension.is_some(),    "plugin.get_extension must be non-null");
    assert!(vtable.on_main_thread.is_some(),   "plugin.on_main_thread must be non-null");
}
