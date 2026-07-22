// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete — P8 entry point.
//!
//! Graph is declared in an instrument YAML file (--instrument=<path>).
//! Hardware devices (Launchpad, Digitakt, Keystep) are opened if present.
//! Terminal UI is started unless --no-tui is passed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use paraclete_antiphon::protocol::{NodeSummary, ParamSummary, TransportSummary};
use paraclete_antiphon::view::ViewRegistry;
use paraclete_antiphon::{AntiphonConfig, AntiphonHandle, AntiphonServer};
use paraclete_app::builder::{build_from_instrument, load_instrument_definition, InstrumentIds};
use paraclete_app::instrument::InstrumentDefinition;
use paraclete_app::project::{load_project, save_project, ProfileBinding, ProjectMetadata};
use paraclete_clap_host::{scan_clap_paths, PluginLibrary};
use paraclete_hal::{
    query_sample_rate, AudioBackend, DigitaktMidiNode, KeystepNode, LaunchpadEmulator,
    LaunchpadNode,
};
use paraclete_nodes::{ScriptEventConsumer, ScriptingGatewayNode};
use paraclete_runtime::NodeConfigurator;
use paraclete_scripting::ScriptingEngine;
use paraclete_theotokos::{TheotokosApp, TheotokosConfig};
use paraclete_tui::{TuiApp, TuiConfig};
use paraclete_view_assembly::{NodeInfo, TrackChain};

const BLOCK_SIZE: usize = 512;

const ID_EMULATOR: u32 = 101;
const ID_LAUNCHPAD: u32 = 102;
const ID_DIGITAKT: u32 = 103;
const ID_KEYSTEP: u32 = 104;
const ID_THEORIA: u32 = 106;
const ID_GW_LP: u32 = 110;
const ID_GW_DT: u32 = 111;
const ID_GW_KS: u32 = 112;
const ID_GW_THEORIA: u32 = 113;

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();

    // ── --recover: fix stranded pipewire-pulse and exit ──────────────────────
    if args.iter().any(|a| a == "--recover") {
        recover_audio_sink();
        return;
    }

    let instrument_path: PathBuf = args
        .iter()
        .find(|a| a.starts_with("--instrument="))
        .and_then(|a| {
            a.split_once('=')
                .map(|(_, v)| v)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| PathBuf::from("instrument.yaml"));

    let load_path: Option<PathBuf> = args
        .iter()
        .find(|a| a.starts_with("--load="))
        .and_then(|a| {
            a.split_once('=')
                .map(|(_, v)| v)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        });

    let save_path: Option<PathBuf> = args
        .iter()
        .find(|a| a.starts_with("--save="))
        .and_then(|a| {
            a.split_once('=')
                .map(|(_, v)| v)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        });

    let no_tui = args.iter().any(|a| a == "--no-tui");
    let mut no_emulator = args.iter().any(|a| a == "--no-emulator");
    let emulator = args.iter().any(|a| a == "--emulator");
    // Theotokos is the default keyboard-first surface (always, unless
    // --no-tui for headless or --emulator for the legacy Launchpad grid).
    // Kept --theotokos as an explicit no-op for compatibility.
    let theotokos = !no_tui && !emulator;
    if theotokos {
        no_emulator = true;
    }
    let dev_ui = args.iter().any(|a| a == "--dev-ui");

    let no_antiphon = args.iter().any(|a| a == "--no-antiphon");
    // Access default flipped 2026-07-10 (user decision): the interface is
    // OPEN on the LAN by default — an instrument you pick up and play, like
    // any hardware box; the protocol only reaches music state today.
    // `--token` opts into the 6-digit session code (untrusted networks);
    // revisit the default when the protocol gains project save/overwrite.
    // `--open` is accepted as a no-op for compatibility with older notes.
    let want_token = args.iter().any(|a| a == "--token");
    let antiphon_port: u16 = args
        .iter()
        .find(|a| a.starts_with("--antiphon-port="))
        .and_then(|a| {
            a.split_once('=')
                .map(|(_, v)| v)
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(paraclete_antiphon::DEFAULT_PORT);
    // `--theoria-dir` overrides whatever the build would otherwise serve
    // (the embedded bundle if `embed-ui` is compiled in, else the on-disk
    // build output directory) — always explicit, never a silent fallback.
    let theoria_dir_override: Option<PathBuf> = args
        .iter()
        .find(|a| a.starts_with("--theoria-dir="))
        .and_then(|a| {
            a.split_once('=')
                .map(|(_, v)| v)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        });

    // ── 1. Load instrument definition ────────────────────────────────────────
    let def = match load_instrument_definition(&instrument_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!(
                "[paraclete] failed to load {}: {e}",
                instrument_path.display()
            );
            std::process::exit(1);
        }
    };
    eprintln!("[paraclete] instrument: {} @ {} BPM", def.name, def.bpm);

    // ── 1.5. Query device sample rate (before graph build) ─────────────────────
    let sample_rate = query_sample_rate().unwrap_or(44100.0);
    if sample_rate != 44100.0 {
        eprintln!("[paraclete] device sample rate: {:.0} Hz", sample_rate);
    }

    // ── 2. Pre-load CLAP plugins (one load per .clap file) ───────────────────
    let libraries: HashMap<String, Arc<PluginLibrary>> = load_plugin_libraries(&def);

    // ── 3. Build node graph ───────────────────────────────────────────────────
    let mut conf = NodeConfigurator::new(sample_rate, BLOCK_SIZE);
    let ids = match build_from_instrument(&def, &mut conf, &libraries) {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("[paraclete] graph build failed: {e}");
            std::process::exit(1);
        }
    };

    // ── 4. Hardware devices ───────────────────────────────────────────────────
    let launchpad_id = try_open_launchpad(&mut conf, no_emulator);
    let digitakt_id = try_open_digitakt(&mut conf);
    let keystep_id = try_open_keystep(&mut conf);

    let mut consumer_lp: Option<ScriptEventConsumer> = None;
    if let Some(lp_dev_id) = launchpad_id {
        let (gw_lp, cons) = ScriptingGatewayNode::new(lp_dev_id, 512);
        conf.add_node(ID_GW_LP, Box::new(gw_lp));
        conf.connect(lp_dev_id, 0, ID_GW_LP, 0).ok();
        consumer_lp = Some(cons);
    }

    let mut consumer_dt: Option<ScriptEventConsumer> = None;
    if let Some(did) = digitakt_id {
        let (gw_dt, cons) = ScriptingGatewayNode::new(did, 256);
        conf.add_node(ID_GW_DT, Box::new(gw_dt));
        conf.connect(did, 0, ID_GW_DT, 0).ok();
        consumer_dt = Some(cons);
    }

    let mut consumer_ks: Option<ScriptEventConsumer> = None;
    if let Some(kid) = keystep_id {
        let (gw_ks, cons) = ScriptingGatewayNode::new(kid, 256);
        conf.add_node(ID_GW_KS, Box::new(gw_ks));
        conf.connect(kid, 0, ID_GW_KS, 0).ok();
        consumer_ks = Some(cons);
    }

    // ── 5. Project load (before executor — nodes still in configurator) ──────
    if let Some(ref path) = load_path {
        match load_project(path, &mut conf) {
            Ok(warnings) => {
                for w in &warnings {
                    eprintln!("[paraclete] WARN: {w}");
                }
                eprintln!("[paraclete] project loaded: {}", path.display());
            }
            Err(e) => eprintln!("[paraclete] load failed: {e}"),
        }
    }

    // ── 6. Project save (before executor — nodes still in configurator) ──────
    if let Some(ref path) = save_path {
        let meta = ProjectMetadata {
            name: path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("paraclete")
                .to_string(),
            bpm: def.bpm as f32,
            created: String::new(),
        };
        let profiles = ProfileBinding { active: vec![] };
        if let Err(e) = save_project(path, &conf, meta, profiles) {
            eprintln!("[paraclete] save failed: {e}");
        } else {
            eprintln!("[paraclete] project saved: {}", path.display());
        }
    }

    // ── 6.5 Antiphon interface server (Theoria surfaces, ADR-031) ────────────
    // After project load so the welcome snapshot reflects restored topology;
    // after save so runtime surface/gateway nodes stay out of project files.
    let mut consumer_theoria: Option<ScriptEventConsumer> = None;
    let mut antiphon: Option<AntiphonHandle> = None;
    if !no_antiphon {
        let summaries = collect_node_summaries(&conf, &ids);
        let static_source = theoria_static_source(theoria_dir_override.clone());
        let config = AntiphonConfig {
            port: antiphon_port,
            token: if want_token {
                load_or_create_token()
            } else {
                String::new()
            },
            static_dir: Some(static_source),
            device_id: ID_THEORIA,
        };
        // Static snapshot: InternalClock auto-starts, so playing=true is
        // truthful at W0. The live state mirror replaces this at W1.
        let transport = TransportSummary {
            playing: true,
            bpm: def.bpm,
        };
        match AntiphonServer::spawn(
            config,
            summaries.clone(),
            transport,
            build_view_registry(&conf, &summaries),
        ) {
            Ok((node, handle)) => {
                conf.add_surface(ID_THEORIA, Box::new(node));
                let (gw, cons) = ScriptingGatewayNode::new(ID_THEORIA, 256);
                conf.add_node(ID_GW_THEORIA, Box::new(gw));
                conf.connect(ID_THEORIA, 0, ID_GW_THEORIA, 0).ok();
                consumer_theoria = Some(cons);
                eprintln!("[paraclete] Theoria: {}", handle.url);
                // Tablet path: the bare URL serves a code-entry screen, so a
                // human types 6 digits instead of a query string.
                if let Some((base, code)) = handle.url.split_once("?t=") {
                    eprintln!("[paraclete]   tablet: open {base} and enter code {code}");
                }
                // mDNS name works on any link the two machines share (house
                // Wi-Fi, Mac-hosted network, direct ethernet cable with
                // link-local addresses) — unlike the routed-IP guess above.
                if let Some(host) = mdns_hostname() {
                    eprintln!("[paraclete]   by name: http://{host}:{antiphon_port}/");
                }
                antiphon = Some(handle);
            }
            Err(e) => eprintln!("[paraclete] antiphon disabled ({e})"),
        }
    }

    // ── 7. Collect capability documents (before executor moves nodes out) ────
    let cap_docs = ids
        .all
        .iter()
        .filter_map(|(_, node_id)| conf.get_node_cap_doc(*node_id).map(|doc| (*node_id, doc)))
        .collect::<HashMap<_, _>>();

    // ── 8. Build executor + start audio ──────────────────────────────────────
    let bus_handle = conf.state_bus_handle();
    let executor = conf.build_executor();

    // Recover from any previous PipeWire stranding before opening ALSA.
    #[cfg(target_os = "linux")]
    recover_audio_sink();

    let _audio = match AudioBackend::start(executor) {
        Ok(b) => {
            eprintln!("[paraclete] audio running — Esc or Ctrl-C to stop");
            b
        }
        Err(e) => {
            eprintln!("[paraclete] audio backend error: {e}");
            std::process::exit(1);
        }
    };

    // ── 9. Scripting setup ────────────────────────────────────────────────────
    let mut scripting = ScriptingEngine::new();
    scripting.bind_state_bus(bus_handle.clone());

    let constants = build_constants(launchpad_id, digitakt_id, keystep_id, &ids);

    for profile_path in &def.profiles {
        let label = Path::new(profile_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(profile_path);
        if let Err(e) = scripting.eval_file(label, profile_path, &constants) {
            eprintln!("[paraclete] profile {profile_path} error: {e}");
        } else {
            eprintln!("[paraclete] profile {profile_path} loaded");
        }
    }

    // ── 10. Macro pre-population ──────────────────────────────────────────────
    for macro_def in &def.macros {
        let script = format!(
            r#"publish_context("encoder_{}", {}, "{}");"#,
            macro_def.encoder, macro_def.node, macro_def.param
        );
        if let Err(e) = scripting.eval_str(&script) {
            eprintln!("[paraclete] macro eval error: {e}");
        }
    }

    // ── 10.5 Contextual encoders (w1-interfaces §Commit 4; s1 exit crit. 2) ──
    // The selected track's voice params ride the encoder context: whenever
    // the profile's /script/lp/selected changes, the app republishes
    // /context/encoder_* for that track's generator straight from its
    // capability document. The app is the layer that has both the cap docs
    // and the track→generator map; profiles stay device-generic and clients
    // just render the context they are mirrored. Instrument-file macros
    // pre-populate the slots and stay authoritative until the first
    // selection change; with no macros the slots follow selection from the
    // first main-loop iteration (the profile writes selected=0 at load).
    const ENCODER_SLOTS: usize = 8;
    let track_encoder_params: Vec<(u32, Vec<String>)> = ids
        .generators
        .iter()
        .map(|gid| {
            let names = cap_docs
                .get(gid)
                .map(|doc| {
                    doc.params
                        .iter()
                        .take(ENCODER_SLOTS)
                        .map(|p| p.name.as_str().to_string())
                        .collect()
                })
                .unwrap_or_default();
            (*gid, names)
        })
        .collect();
    let mut published_context_track: Option<i64> = None;
    // A macro-less instrument opts into selection-following context; an
    // instrument with explicit macros keeps them (they were chosen by hand).
    let auto_context = def.macros.is_empty() && !track_encoder_params.is_empty();

    // ── 11. Graceful shutdown signal ──────────────────────────────────────────
    let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let r = std::sync::Arc::clone(&running);
    ctrlc::set_handler(move || {
        r.store(false, std::sync::atomic::Ordering::SeqCst);
    })
    .ok();

    // ── 12. TUI / Theotokos setup ──────────────────────────────────────────────
    type CrosstermTerminal = ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>;
    let mut tui_opt: Option<(TuiApp, CrosstermTerminal)> = None;
    let mut theotokos_opt: Option<(TheotokosApp, CrosstermTerminal)> = None;
    if theotokos {
        match setup_terminal() {
            Ok(terminal) => {
                let gen_names: Vec<String> = ids
                    .generators
                    .iter()
                    .map(|id| {
                        cap_docs
                            .get(id)
                            .map(|d| d.name.to_string())
                            .unwrap_or_else(|| format!("Node{}", id))
                    })
                    .collect();
                match TheotokosApp::new(TheotokosConfig {
                    clock_id: ids.clock,
                    seq_ids: ids.sequencers.clone(),
                    gen_ids: ids.generators.clone(),
                    gen_names,
                    caps: cap_docs.clone(),
                    fps: 30,
                }) {
                    Ok(app) => {
                        // TK1 C0: initial /script/theotokos/selected publish
                        if let Some(first_seq) = ids.sequencers.first() {
                            let mut bus_mut = bus_handle.borrow_mut();
                            bus_mut.write(
                                "/script/theotokos/selected",
                                paraclete_node_api::StateBusValue::Int(*first_seq as i64),
                            );
                        }
                        theotokos_opt = Some((app, terminal));
                    }
                    Err(e) => eprintln!("[paraclete] Theotokos setup failed: {e}"),
                }
            }
            Err(e) => eprintln!("[paraclete] Theotokos terminal setup failed: {e}"),
        }
    } else if emulator && !no_tui {
        let tui_config = TuiConfig {
            clock_id: ids.clock,
            seq_ids: ids.sequencers.clone(),
            encoder_count: 8,
            fps: 30,
        };
        match setup_terminal() {
            Ok(terminal) => {
                tui_opt = Some((
                    TuiApp::new(bus_handle.clone(), tui_config, cap_docs),
                    terminal,
                ));
            }
            Err(e) => eprintln!("[paraclete] TUI setup failed: {e}"),
        }
    }

    // ── 13. Main loop ─────────────────────────────────────────────────────────
    let mut event_buf: Vec<paraclete_node_api::SurfaceEventMsg> = Vec::with_capacity(64);
    let mut dev_ui_tick = 0u64;
    // Monotonic clock for the Antiphon state mirror (w1-interfaces.md §Commit
    // 2) — antiphon does no clock reads of its own; the caller passes now_ms.
    let loop_clock = std::time::Instant::now();

    while running.load(std::sync::atomic::Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(1));

        conf.process_main_thread();

        // Step 1.4: contextual encoders — follow the profile's selected
        // track (see step 10.5 for the policy). Before the mirror pump so a
        // selection change and its new context leave in the same flush.
        if auto_context {
            let selected = match bus_handle.borrow().read("/script/lp/selected") {
                Some(paraclete_node_api::StateBusValue::Int(i)) => Some(*i),
                Some(paraclete_node_api::StateBusValue::Float(f)) => Some(*f as i64),
                _ => None,
            };
            if let Some(t) = selected {
                if published_context_track != Some(t) && t >= 0 {
                    if let Some((gid, names)) = track_encoder_params.get(t as usize) {
                        published_context_track = Some(t);
                        let mut bus = bus_handle.borrow_mut();
                        for i in 0..ENCODER_SLOTS {
                            let (node, param) = match names.get(i) {
                                Some(name) => (*gid as i64, name.clone()),
                                // Slot beyond this machine's param count:
                                // cleared (clients render it unmapped).
                                None => (0, String::new()),
                            };
                            bus.write(
                                &format!("/context/encoder_{i}/node"),
                                paraclete_node_api::StateBusValue::Int(node),
                            );
                            bus.write(
                                &format!("/context/encoder_{i}/param"),
                                paraclete_node_api::StateBusValue::Text(param),
                            );
                        }
                    }
                }
            }
        }

        // Step 1.5: state/context mirror pump (after process_main_thread so
        // the bus reflects this cycle's executor updates).
        if let Some(handle) = antiphon.as_mut() {
            let now_ms = loop_clock.elapsed().as_millis() as u64;
            handle.pump(&bus_handle.borrow(), now_ms);
        }

        event_buf.clear();
        if let Some(ref mut c) = consumer_lp {
            c.drain(&mut event_buf);
        }
        if let Some(ref mut c) = consumer_dt {
            c.drain(&mut event_buf);
        }
        if let Some(ref mut c) = consumer_ks {
            c.drain(&mut event_buf);
        }
        if let Some(ref mut c) = consumer_theoria {
            c.drain(&mut event_buf);
        }

        for ev in &event_buf {
            scripting.dispatch_surface_event(ev);
        }

        scripting.process_subscriptions(&bus_handle);

        for cmd in scripting.take_pending_commands() {
            conf.send_command(cmd).ok();
        }
        // Semantic-plane commands (set_param/bump_param/node_cmd) resolved by
        // Antiphon client threads (w1-interfaces.md §Commit 3).
        if let Some(h) = antiphon.as_ref() {
            for cmd in h.drain_commands() {
                conf.send_command(cmd).ok();
            }
        }

        let mut led_output = scripting.take_pending_output();
        // Mirror LED output addressed to the Launchpad/emulator onto the
        // Theoria surface so both show the same state (w0-interfaces §wiring).
        if antiphon.is_some() {
            if let Some(lp_id) = launchpad_id {
                if let Some(mut lp_out) = led_output.get(&lp_id).cloned() {
                    led_output
                        .entry(ID_THEORIA)
                        .and_modify(|o| {
                            // Mirrored updates first: downstream is last-write-wins,
                            // so a profile's direct-to-Theoria write beats the mirror.
                            std::mem::swap(&mut lp_out.led_updates, &mut o.led_updates);
                            o.led_updates.append(&mut lp_out.led_updates);
                        })
                        .or_insert(lp_out);
                }
            }
        }
        if !led_output.is_empty() {
            conf.deliver_script_output(led_output);
        }

        if let Some((ref mut tk, ref mut terminal)) = theotokos_opt {
            let now_ms = loop_clock.elapsed().as_millis() as u64;
            if let Err(e) = tk.tick(terminal, &bus_handle, now_ms) {
                eprintln!("[paraclete] Theotokos error: {e}");
            }
            for cmd in tk.take_pending_commands() {
                conf.send_command(cmd).ok();
            }
            if tk.should_quit() {
                running.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }

        if let Some((ref mut tui, ref mut terminal)) = tui_opt {
            if let Err(e) = tui.tick(terminal) {
                eprintln!("[paraclete] TUI error: {e}");
            }
        }

        if dev_ui {
            dev_ui_tick += 1;
            if dev_ui_tick % 1000 == 0 {
                for seq_id in &ids.sequencers {
                    let step = conf.state_bus_read(&format!("/node/{seq_id}/state/current_step"));
                    let steps = conf.state_bus_read(&format!("/node/{seq_id}/state/steps"));
                    eprintln!("[dev-ui] seq={seq_id} step={step:?} pattern={steps:?}");
                }
            }
        }
    }

    // ── 14. Shutdown ──────────────────────────────────────────────────────────
    if let Some((tk, terminal)) = theotokos_opt {
        tk.shutdown().ok();
        restore_terminal(terminal).ok();
    } else if let Some((tui, terminal)) = tui_opt {
        tui.shutdown().ok();
        restore_terminal(terminal).ok();
    }
    eprintln!("[paraclete] stopped.");

    // PipeWire may strand on auto_null after Paraclete's direct ALSA access
    // closes.  Restart only pipewire-pulse (not the full pipewire daemon) —
    // this re-discovers the ALSA sink without killing other applications.
    #[cfg(target_os = "linux")]
    recover_audio_sink();
}

/// Load CLAP plugin libraries needed by the instrument definition.
/// Each .clap file is loaded at most once. Matching libraries are wrapped in
/// `Arc` and inserted for every matching plugin ID in that file (supporting
/// multi-plugin bundles). Non-matching files are loaded once for descriptor
/// enumeration then dropped.
fn load_plugin_libraries(def: &InstrumentDefinition) -> HashMap<String, Arc<PluginLibrary>> {
    let mut libraries: HashMap<String, Arc<PluginLibrary>> = HashMap::new();

    // First: plugins with an explicit plugin_path — load directly without scanning.
    for node_def in &def.nodes {
        if node_def.type_tag != "clap_plugin" {
            continue;
        }
        let Some(plugin_id) = node_def.plugin_id.as_deref() else {
            continue;
        };
        if libraries.contains_key(plugin_id) {
            continue;
        }
        if let Some(explicit_path) = node_def.plugin_path.as_deref() {
            match PluginLibrary::load(Path::new(explicit_path)) {
                Ok(lib) => {
                    eprintln!("[paraclete] CLAP plugin loaded: {plugin_id}");
                    libraries.insert(plugin_id.to_string(), Arc::new(lib));
                }
                Err(e) => eprintln!("[paraclete] CLAP load failed ({plugin_id}): {e}"),
            }
        }
    }

    // Second: plugins without an explicit path — scan OS-standard directories.
    // Each .clap file is loaded once; Arc-cloned into the map for every matching
    // plugin ID so multi-plugin bundles work correctly.
    let unresolved: Vec<&str> = def
        .nodes
        .iter()
        .filter(|n| n.type_tag == "clap_plugin" && n.plugin_path.is_none())
        .filter_map(|n| n.plugin_id.as_deref())
        .filter(|id| !libraries.contains_key(*id))
        .collect();

    if !unresolved.is_empty() {
        for clap_path in scan_clap_paths() {
            if let Ok(lib) = PluginLibrary::load(&clap_path) {
                let matched: Vec<String> = lib
                    .descriptors()
                    .iter()
                    .filter(|d| unresolved.contains(&d.id.as_str()))
                    .map(|d| d.id.clone())
                    .collect();
                if !matched.is_empty() {
                    let lib = Arc::new(lib);
                    for id in matched {
                        libraries.entry(id.clone()).or_insert_with(|| {
                            eprintln!("[paraclete] CLAP plugin found via scan: {id}");
                            Arc::clone(&lib)
                        });
                    }
                }
            }
        }
    }

    libraries
}

/// Load the Antiphon session token from `.antiphon-token` (CWD), creating it
/// on first run. Persisting across restarts is what makes the client's
/// auto-reconnect-after-app-restart work — a per-run token would bounce every
/// reconnecting client with `bye "bad token"`. Delete the file to rotate.
/// (`fastrand` is not a CSPRNG; acceptable under the recorded W0 LAN posture.)
/// The built Theoria client bundle (`web/packages/app/dist`), baked into the
/// binary at compile time when the `embed-ui` feature is on
/// (w1-interfaces.md §Commit 4). Requires `npm run build` in `web/` to have
/// produced the `dist/` directory before this crate is compiled with the
/// feature enabled.
#[cfg(feature = "embed-ui")]
static EMBEDDED_THEORIA_UI: include_dir::Dir<'static> =
    include_dir::include_dir!("$CARGO_MANIFEST_DIR/../../web/packages/app/dist");

/// Resolve where the Theoria HTTP thread should read the client bundle from.
/// `--theoria-dir` always wins (dev override). Otherwise: the embedded
/// bundle if this binary was built with `embed-ui`, else the on-disk build
/// output directory (dev default without the feature — run `npm run build`
/// in `web/` first).
fn theoria_static_source(dir_override: Option<PathBuf>) -> paraclete_antiphon::http::StaticSource {
    use paraclete_antiphon::http::StaticSource;
    if let Some(dir) = dir_override {
        return StaticSource::Disk(dir);
    }
    #[cfg(feature = "embed-ui")]
    {
        StaticSource::Embedded(&EMBEDDED_THEORIA_UI)
    }
    #[cfg(not(feature = "embed-ui"))]
    {
        StaticSource::Disk(PathBuf::from("web/packages/app/dist"))
    }
}

/// Best-effort `<name>.local` for the printed URL. Resolvable via mDNS on
/// any shared link regardless of which interface routes (the `lan_ip()`
/// guess assumes a 192.168/16 route and is wrong on e.g. a direct ethernet
/// cable with link-local addressing).
fn mdns_hostname() -> Option<String> {
    // macOS: the Bonjour name is authoritative (`hostname` often returns a
    // DHCP FQDN like "MacBookAir.ht.home" that mDNS will not resolve).
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
    {
        if out.status.success() {
            if let Ok(name) = String::from_utf8(out.stdout) {
                let name = name.trim();
                if !name.is_empty() {
                    return Some(format!("{name}.local"));
                }
            }
        }
    }
    // Fallback: first label of the hostname (avahi and friends publish it).
    let out = std::process::Command::new("hostname").output().ok()?;
    let name = String::from_utf8(out.stdout).ok()?;
    let name = name.trim().split('.').next()?.trim();
    if name.is_empty() {
        return None;
    }
    Some(format!("{name}.local"))
}

fn load_or_create_token() -> String {
    const TOKEN_FILE: &str = ".antiphon-token";
    if let Ok(existing) = std::fs::read_to_string(TOKEN_FILE) {
        let existing = existing.trim().to_ascii_lowercase();
        // Human-typeable 6-digit session code (2026-07-10, tablet ergonomics:
        // a 32-hex token cannot be typed on glass; the client now offers a
        // code-entry screen at the bare URL). Deliberate posture trade under
        // the recorded W0 home-LAN threat model — `--open` remains the
        // zero-token option, and deleting the file rotates the code.
        if existing.len() == 6 && existing.bytes().all(|b| b.is_ascii_digit()) {
            return existing;
        }
        eprintln!("[paraclete] {TOKEN_FILE} is not a 6-digit code; rotating to the tablet-typeable format");
    }
    let token = format!("{:06}", fastrand::u32(0..1_000_000));
    if let Err(e) = std::fs::write(TOKEN_FILE, &token) {
        eprintln!("[paraclete] could not persist {TOKEN_FILE} ({e}); token is per-run");
    }
    token
}

/// Assemble the `welcome` node snapshot from the configurator's cap-doc cache.
/// Antiphon never talks to the configurator directly (w0 spec §kerygma).
fn collect_node_summaries(conf: &NodeConfigurator, ids: &InstrumentIds) -> Vec<NodeSummary> {
    ids.all
        .iter()
        .filter_map(|(label, node_id)| {
            let doc = conf.get_node_cap_doc(*node_id)?;
            let type_tag = conf.type_tag_for(*node_id).unwrap_or("").to_string();
            // Prefer the instrument-file display_name (classify_node falls
            // back to the type_tag when none was given) — it is the human
            // name ("Kick", "Snare") that clients label tracks with
            // (s1.md F8/F9); the cap-doc name is a per-type constant.
            let name = if !label.is_empty() && *label != type_tag {
                label.clone()
            } else {
                doc.name.to_string()
            };
            Some(NodeSummary {
                id: *node_id,
                type_tag,
                name,
                has_view: doc.view.is_some(),
                params: doc
                    .params
                    .iter()
                    .map(|p| ParamSummary {
                        id: p.id,
                        name: p.name.as_str().to_string(),
                        min: p.min,
                        max: p.max,
                        default: p.default,
                    })
                    .collect(),
            })
        })
        .collect()
}

fn build_view_registry(conf: &NodeConfigurator, summaries: &[NodeSummary]) -> ViewRegistry {
    use paraclete_node_api::{CapabilityDocument, PortType};

    let mut rules: HashMap<u32, paraclete_node_api::Rule> = HashMap::new();

    // Build NodeInfo from cap-docs + instrument labels.
    let mut node_infos: HashMap<u32, NodeInfo> = HashMap::new();
    for s in summaries {
        if let Some(doc) = conf.get_node_cap_doc(s.id) {
            if let Some(rule) = doc.view.clone() {
                rules.insert(s.id, rule);
            }
            let params: Vec<(u32, String)> = doc
                .params
                .iter()
                .map(|p| (p.id, p.name.to_string()))
                .collect();
            node_infos.insert(
                s.id,
                NodeInfo {
                    display_name: if s.name != doc.name.as_ref() {
                        Some(s.name.clone())
                    } else {
                        None
                    },
                    params,
                },
            );
        }
    }

    // Edge-derived chains: follow audio edges from each engine to the first
    // mix/audio_output, collecting rule-bearing chain nodes.  Also derive
    // sequencer→engine event-edge pairs for the track map.
    let edges: Vec<(u32, u32)> = conf.all_edges().map(|e| (e.src_node, e.dst_node)).collect();

    // Build a quick lookup: node_id -> cap_doc (for port-type checks).
    let caps: HashMap<u32, CapabilityDocument> = summaries
        .iter()
        .filter_map(|s| conf.get_node_cap_doc(s.id).map(|d| (s.id, d)))
        .collect();

    fn is_audio_in(caps: &HashMap<u32, CapabilityDocument>, nid: u32) -> bool {
        caps.get(&nid)
            .map(|d| {
                d.ports.iter().any(|p| {
                    p.port_type == PortType::Audio
                        && p.direction == paraclete_node_api::PortDirection::Input
                })
            })
            .unwrap_or(false)
    }

    fn is_audio_out(caps: &HashMap<u32, CapabilityDocument>, nid: u32) -> bool {
        caps.get(&nid)
            .map(|d| {
                d.ports.iter().any(|p| {
                    p.port_type == PortType::Audio
                        && p.direction == paraclete_node_api::PortDirection::Output
                })
            })
            .unwrap_or(false)
    }

    // TK1 C2: include clap_plugin engines.
    let engine_type_tags: &[&str] = &[
        "analog_engine:kick",
        "analog_engine:snare",
        "analog_engine:hihat",
        "fm_engine:kick",
        "fm_engine:bell",
        "fm_engine:bass",
        "sampler",
        "clap_plugin",
    ];
    let engine_ids: Vec<u32> = summaries
        .iter()
        .filter(|s| engine_type_tags.contains(&s.type_tag.as_str()))
        .map(|s| s.id)
        .collect();

    // Deduplicate with any node that has audio output and a view Rule
    // (covers engines whose type_tag isn't in the explicit list, including
    // clap_plugin — review m10).
    let mut engine_set: std::collections::BTreeSet<u32> = engine_ids.into_iter().collect();
    for (nid, _rule) in &rules {
        if is_audio_out(&caps, *nid) {
            engine_set.insert(*nid);
        }
    }
    let engine_ids: Vec<u32> = engine_set.into_iter().collect();

    // Audio-edge traversal: follow audio out → audio in edges from each engine,
    // stopping before mix/audio_output.  This is a BFS over the small graph.
    let mix_ids: std::collections::HashSet<u32> = summaries
        .iter()
        .filter(|s| s.type_tag == "mix" || s.type_tag == "audio_output")
        .map(|s| s.id)
        .collect();

    let mut chains: Vec<TrackChain> = Vec::new();
    for &engine_id in &engine_ids {
        let mut chain_ids = Vec::new();
        let mut visited = std::collections::HashSet::new();
        visited.insert(engine_id);
        let mut frontier: Vec<u32> = vec![engine_id];
        while let Some(current) = frontier.pop() {
            for &(src, tgt) in &edges {
                if src == current && !visited.contains(&tgt) && !mix_ids.contains(&tgt) {
                    if is_audio_out(&caps, src) && is_audio_in(&caps, tgt) {
                        visited.insert(tgt);
                        // Only include chain nodes that have a view Rule.
                        if rules.contains_key(&tgt) {
                            chain_ids.push(tgt);
                        }
                        frontier.push(tgt);
                    }
                }
            }
        }
        chains.push(TrackChain {
            engine_node_id: engine_id,
            chain_ids,
        });
    }

    ViewRegistry {
        rules,
        chains,
        node_infos,
    }
}

fn build_constants(
    lp_dev_id: Option<u32>,
    digitakt_id: Option<u32>,
    keystep_id: Option<u32>,
    ids: &InstrumentIds,
) -> Vec<(String, rhai::Dynamic)> {
    fn id_array(ids: &[u32]) -> rhai::Dynamic {
        rhai::Dynamic::from(
            ids.iter()
                .map(|&id| rhai::Dynamic::from(id as i64))
                .collect::<Vec<_>>(),
        )
    }
    vec![
        (
            "LP_DEVICE_ID".into(),
            rhai::Dynamic::from(lp_dev_id.unwrap_or(0) as i64),
        ),
        (
            "DT_DEVICE_ID".into(),
            rhai::Dynamic::from(digitakt_id.unwrap_or(0) as i64),
        ),
        (
            "KS_DEVICE_ID".into(),
            rhai::Dynamic::from(keystep_id.unwrap_or(0) as i64),
        ),
        // Injected even with --no-antiphon so profiles referencing it still load.
        (
            "THEORIA_DEVICE_ID".into(),
            rhai::Dynamic::from(ID_THEORIA as i64),
        ),
        ("CLOCK_ID".into(), rhai::Dynamic::from(ids.clock as i64)),
        ("TRACK_SEQ_IDS".into(), id_array(&ids.sequencers)),
        ("TRACK_SAMP_IDS".into(), id_array(&ids.samplers)),
        ("TRACK_GEN_IDS".into(), id_array(&ids.generators)),
        ("TRACK_DIST_IDS".into(), id_array(&ids.distortions)),
        ("TRACK_FILT_IDS".into(), id_array(&ids.filters)),
    ]
}

fn setup_terminal(
) -> Result<ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>, std::io::Error>
{
    use crossterm::execute;
    use crossterm::terminal::{enable_raw_mode, EnterAlternateScreen};
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    ratatui::Terminal::new(backend)
}

fn restore_terminal(
    mut terminal: ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
) -> Result<(), std::io::Error> {
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, LeaveAlternateScreen};
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn try_open_launchpad(conf: &mut NodeConfigurator, no_emulator: bool) -> Option<u32> {
    match LaunchpadNode::open() {
        Ok(node) => {
            conf.add_surface(ID_LAUNCHPAD, Box::new(node));
            eprintln!("[paraclete] Launchpad connected");
            Some(ID_LAUNCHPAD)
        }
        Err(e) => {
            if no_emulator {
                eprintln!("[paraclete] Launchpad not found ({e}), running headless");
                None
            } else {
                eprintln!("[paraclete] Launchpad not found ({e}), using terminal emulator");
                conf.add_surface(ID_EMULATOR, Box::new(LaunchpadEmulator::new()));
                Some(ID_EMULATOR)
            }
        }
    }
}

fn try_open_digitakt(conf: &mut NodeConfigurator) -> Option<u32> {
    match DigitaktMidiNode::open() {
        Ok(node) => {
            conf.add_surface(ID_DIGITAKT, Box::new(node));
            eprintln!("[paraclete] Digitakt connected");
            Some(ID_DIGITAKT)
        }
        Err(_) => None,
    }
}

fn try_open_keystep(conf: &mut NodeConfigurator) -> Option<u32> {
    match KeystepNode::open() {
        Ok(node) => {
            conf.add_surface(ID_KEYSTEP, Box::new(node));
            eprintln!("[paraclete] Keystep connected");
            Some(ID_KEYSTEP)
        }
        Err(_) => None,
    }
}

#[cfg(target_os = "linux")]
fn recover_audio_sink() {
    let sinks = std::process::Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    if sinks.contains("alsa_output") {
        return;
    }

    eprintln!("[paraclete] audio sink missing — restarting pipewire");
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "restart", "pipewire", "pipewire-pulse"])
        .output();
}

#[cfg(not(target_os = "linux"))]
fn recover_audio_sink() {}
