use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use paraclete_node_api::{
    capability::{CapabilityDocument, ParamDescriptor, ParamUnit},
    port::{PortDescriptor, PortDirection, PortType},
    StateBusHandle, StateBusValue,
};
use paraclete_tui::{TuiApp, TuiConfig};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

fn make_terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(80, 24)).unwrap()
}

fn make_bus() -> Rc<RefCell<StateBusHandle>> {
    Rc::new(RefCell::new(StateBusHandle::new()))
}

fn make_config(clock_id: u32, seq_ids: Vec<u32>) -> TuiConfig {
    TuiConfig {
        clock_id,
        seq_ids,
        encoder_count: 8,
        fps: 30,
    }
}

fn make_cap_doc_with_cutoff() -> CapabilityDocument {
    CapabilityDocument {
        name: "TestNode".into(),
        vendor: "test".into(),
        version: (0, 1, 0),
        ports: vec![PortDescriptor {
            id: 0,
            name: "audio_out".into(),
            direction: PortDirection::Output,
            port_type: PortType::Audio,
        }],
        params: vec![ParamDescriptor {
            id: ParamDescriptor::id_for_name("cutoff"),
            name: "cutoff".into(),
            min: 20.0,
            max: 20000.0,
            default: 1000.0,
            stepped: false,
            unit: ParamUnit::Hz,
            display: None,
        }],
        extensions: vec![],
    }
}

#[test]
fn tui_state_updates_bpm_from_state_bus() {
    let bus = make_bus();
    bus.borrow_mut().write("/transport/bpm", StateBusValue::Float(140.0));

    let config = make_config(1, vec![]);
    let mut app = TuiApp::new(bus, config, HashMap::new());
    let mut terminal = make_terminal();
    app.tick_with_time(&mut terminal, 1000).unwrap();

    assert_eq!(app.state.bpm, 140.0);
}

#[test]
fn tui_state_playing_flag_reflects_state_bus() {
    let bus = make_bus();
    bus.borrow_mut().write("/transport/playing", StateBusValue::Bool(true));

    let config = make_config(1, vec![]);
    let mut app = TuiApp::new(bus.clone(), config, HashMap::new());
    let mut terminal = make_terminal();
    app.tick_with_time(&mut terminal, 1000).unwrap();
    assert!(app.state.playing);

    bus.borrow_mut().write("/transport/playing", StateBusValue::Bool(false));
    app.tick_with_time(&mut terminal, 1001).unwrap();
    assert!(!app.state.playing);
}

#[test]
fn tui_encoder_slot_resolves_param_label_from_cap_doc() {
    let bus = make_bus();
    {
        let mut b = bus.borrow_mut();
        b.write("/context/encoder_0/node",  StateBusValue::Float(42.0));
        b.write("/context/encoder_0/param", StateBusValue::Float(
            ParamDescriptor::id_for_name("cutoff") as f64
        ));
        b.write("/node/42/param/cutoff", StateBusValue::Float(1200.0));
    }

    let mut cap_docs = HashMap::new();
    cap_docs.insert(42u32, make_cap_doc_with_cutoff());

    let config = TuiConfig {
        clock_id: 1,
        seq_ids: vec![],
        encoder_count: 1,
        fps: 30,
    };

    let mut app = TuiApp::new(bus, config, cap_docs);
    let mut terminal = make_terminal();
    app.tick_with_time(&mut terminal, 1000).unwrap();

    assert_eq!(app.state.encoders[0].label, "cutoff");
    assert_eq!(app.state.encoders[0].value, 1200.0);
    assert_eq!(app.state.encoders[0].min, 20.0);
    assert_eq!(app.state.encoders[0].max, 20000.0);
}

#[test]
fn tui_recently_changed_clears_after_500ms() {
    let bus = make_bus();
    {
        let mut b = bus.borrow_mut();
        b.write("/context/encoder_0/node",  StateBusValue::Float(42.0));
        b.write("/context/encoder_0/param", StateBusValue::Float(
            ParamDescriptor::id_for_name("cutoff") as f64
        ));
        b.write("/node/42/param/cutoff", StateBusValue::Float(1200.0));
    }

    let mut cap_docs = HashMap::new();
    cap_docs.insert(42u32, make_cap_doc_with_cutoff());

    let config = TuiConfig {
        clock_id: 1,
        seq_ids: vec![],
        encoder_count: 1,
        fps: 30,
    };

    let mut app = TuiApp::new(bus.clone(), config, cap_docs);
    let mut terminal = make_terminal();

    // First tick: value changes → recently_changed = true
    app.tick_with_time(&mut terminal, 1000).unwrap();
    assert!(app.state.encoders[0].recently_changed);

    // Second tick at same time: no change to value, but < 500ms elapsed → still true
    app.tick_with_time(&mut terminal, 1100).unwrap();
    assert!(app.state.encoders[0].recently_changed);

    // Third tick at 501ms after change → recently_changed cleared
    app.tick_with_time(&mut terminal, 1502).unwrap();
    assert!(!app.state.encoders[0].recently_changed);
}

#[test]
fn tui_reads_pattern_engine_paths_and_windows_steps() {
    // P10 C5: the pattern/page/speed indicator sources, and the 16-step
    // display window slicing a 64-step bitfield around the playhead.
    let bus = make_bus();
    {
        let mut b = bus.borrow_mut();
        b.write("/node/10/state/current_step",   StateBusValue::Int(20));
        b.write("/node/10/state/pattern_length", StateBusValue::Int(32));
        b.write("/node/10/state/active_pattern", StateBusValue::Int(2));
        b.write("/node/10/state/cued_pattern",   StateBusValue::Int(5));
        b.write("/node/10/state/current_page",   StateBusValue::Int(2));
        b.write("/node/10/state/page_count",     StateBusValue::Int(4));
        b.write("/node/10/state/speed_mult",     StateBusValue::Float(2.0));
        // Steps 16 and 20 active in a 32-step pattern.
        let mut bits = vec!['0'; 32];
        bits[16] = '1';
        bits[20] = '1';
        b.write(
            "/node/10/state/steps",
            StateBusValue::Text(bits.into_iter().collect()),
        );
    }

    let config = make_config(1, vec![10]);
    let mut app = TuiApp::new(bus, config, HashMap::new());
    let mut terminal = make_terminal();
    app.tick_with_time(&mut terminal, 1000).unwrap();

    assert_eq!(app.state.pattern_length, 32);
    assert_eq!(app.state.active_pattern, 2);
    assert_eq!(app.state.cued_pattern, 5);
    assert_eq!(app.state.current_page, 2);
    assert_eq!(app.state.page_count, 4);
    assert_eq!(app.state.speed_mult, 2.0);
    // Playhead at 20 -> window base 16; window-relative steps 0 and 4 active.
    assert_eq!(app.state.window_base, 16);
    let mut expected = [false; 16];
    expected[0] = true;
    expected[4] = true;
    assert_eq!(app.state.steps, expected, "steps sliced to the playhead's window");
}
