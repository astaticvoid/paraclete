mod analysis;
mod baseline;
mod scenario;
mod resolve;
mod wav;

use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paraclete_app::builder::{build_from_instrument, load_instrument_definition};
use paraclete_node_api::{NodeCommand, StateBusValue, CMD_BUMP_PARAM, CMD_SET_PARAM, CMD_TRIGGER};
use paraclete_node_api::capability::ParamDescriptor;
use paraclete_node_api::state_bus::StateBusHandle;
use paraclete_runtime::NodeConfigurator;
use paraclete_nodes::sequencer::Sequencer;

use resolve::NameResolver;
use scenario::{Assertion, Probe, ResolvedActionKind, TestScenario};

const CMD_TOGGLE_STEP: u32 = Sequencer::CMD_TOGGLE_STEP;
const CMD_SET_STEP: u32 = Sequencer::CMD_SET_STEP;
const CMD_CLEAR: u32 = Sequencer::CMD_CLEAR;
const CMD_SET_PATTERN: u32 = Sequencer::CMD_SET_PATTERN;
const CMD_SET_LENGTH: u32 = Sequencer::CMD_SET_LENGTH;
const CMD_SET_SPEED: u32 = Sequencer::CMD_SET_SPEED;
const CMD_SET_PAGE_LOOP: u32 = Sequencer::CMD_SET_PAGE_LOOP;
const CMD_SET_STEP_TIMING: u32 = Sequencer::CMD_SET_STEP_TIMING;
const CMD_SET_FILL_A: u32 = Sequencer::CMD_SET_FILL_A;
const CMD_SET_FILL_B: u32 = Sequencer::CMD_SET_FILL_B;
const CMD_SET_STEP_CONDITION: u32 = Sequencer::CMD_SET_STEP_CONDITION;
const CMD_CHAIN_PUSH: u32 = Sequencer::CMD_CHAIN_PUSH;
const CMD_CHAIN_CLEAR: u32 = Sequencer::CMD_CHAIN_CLEAR;

fn auto_play_command() -> &'static str {
    #[cfg(target_os = "macos")]
    { return "afplay"; }
    #[cfg(target_os = "linux")]
    { "pw-play" }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    { return "afplay"; }
}

const CAPTURE_RING_CAPACITY: usize = 512;

struct CaptureRing {
    blocks: Vec<UnsafeCell<Vec<f32>>>,
    write_idx: AtomicUsize,
}

unsafe impl Send for CaptureRing {}
unsafe impl Sync for CaptureRing {}

impl CaptureRing {
    fn new(capacity: usize) -> Self {
        Self {
            blocks: (0..capacity).map(|_| UnsafeCell::new(Vec::new())).collect(),
            write_idx: AtomicUsize::new(0),
        }
    }

    fn push(&self, block: Vec<f32>) {
        let idx = self.write_idx.load(Ordering::Relaxed) % CAPTURE_RING_CAPACITY;
        let slot = unsafe { &mut *self.blocks[idx].get() };
        slot.clear();
        slot.extend_from_slice(&block);
        self.write_idx.fetch_add(1, Ordering::Release);
    }

    fn drain(&self, out: &mut Vec<f32>, last_read: &mut usize) {
        let end = self.write_idx.load(Ordering::Acquire);
        while *last_read < end {
            let idx = *last_read % CAPTURE_RING_CAPACITY;
            out.extend_from_slice(unsafe { &*self.blocks[idx].get() });
            *last_read += 1;
        }
    }
}

struct TestContext {
    conf: NodeConfigurator,
    #[allow(dead_code)]
    executor: Arc<Mutex<paraclete_runtime::NodeExecutor>>,
    #[allow(dead_code)]
    bus_handle: std::rc::Rc<std::cell::RefCell<StateBusHandle>>,
    capture: Arc<CaptureRing>,
    running: Arc<AtomicBool>,
    resolver: NameResolver,
    sample_rate: f32,
    block_size: usize,
}

fn resolve_target(resolver: &NameResolver, target: &str) -> Result<u32, String> {
    resolver.resolve_required(target)
}

fn dispatch_action(conf: &mut NodeConfigurator, action: &ResolvedActionKind) -> Result<(), String> {
    let cmd = match action {
        ResolvedActionKind::SetParam { target_id, param_name, value } => {
            let param_id = param_id_for_name(param_name);
            NodeCommand { target_id: *target_id, type_id: CMD_SET_PARAM, arg0: param_id as i64, arg1: *value }
        }
        ResolvedActionKind::BumpParam { target_id, param_name, delta } => {
            let param_id = param_id_for_name(param_name);
            NodeCommand { target_id: *target_id, type_id: CMD_BUMP_PARAM, arg0: param_id as i64, arg1: *delta }
        }
        ResolvedActionKind::Trigger { target_id, note, velocity } => {
            NodeCommand { target_id: *target_id, type_id: CMD_TRIGGER, arg0: *note, arg1: *velocity }
        }
        ResolvedActionKind::ToggleStep { target_id, step } => {
            NodeCommand { target_id: *target_id, type_id: CMD_TOGGLE_STEP, arg0: *step, arg1: 0.0 }
        }
        ResolvedActionKind::SetStep { target_id, step, note } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_STEP, arg0: *step, arg1: *note as f64 }
        }
        ResolvedActionKind::Clear { target_id } => {
            NodeCommand { target_id: *target_id, type_id: CMD_CLEAR, arg0: 0, arg1: 0.0 }
        }
        ResolvedActionKind::SetPattern { target_id, pattern } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_PATTERN, arg0: *pattern, arg1: 0.0 }
        }
        ResolvedActionKind::SetLength { target_id, steps } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_LENGTH, arg0: *steps, arg1: -1.0 }
        }
        ResolvedActionKind::SetSpeed { target_id, speed } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_SPEED, arg0: 0, arg1: *speed }
        }
        ResolvedActionKind::SetPageLoop { target_id, start_page, end_page } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_PAGE_LOOP, arg0: *start_page, arg1: *end_page as f64 }
        }
        ResolvedActionKind::SetStepTiming { target_id, step, micro_offset } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_STEP_TIMING, arg0: *step, arg1: *micro_offset as f64 }
        }
        ResolvedActionKind::SetFillA { target_id, active } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_FILL_A, arg0: if *active { 1 } else { 0 }, arg1: 0.0 }
        }
        ResolvedActionKind::SetFillB { target_id, active } => {
            NodeCommand { target_id: *target_id, type_id: CMD_SET_FILL_B, arg0: if *active { 1 } else { 0 }, arg1: 0.0 }
        }
        ResolvedActionKind::SetStepCondition { target_id, step, probability, repeat_n, repeat_m, fill } => {
            let packed: u64 =
                (*probability as u64) |
                ((*repeat_n as u64) << 8) |
                ((*repeat_m as u64) << 16) |
                ((*fill as u64) << 24);
            NodeCommand { target_id: *target_id, type_id: CMD_SET_STEP_CONDITION, arg0: *step, arg1: packed as f64 }
        }
        ResolvedActionKind::ChainPush { target_id, pattern } => {
            NodeCommand { target_id: *target_id, type_id: CMD_CHAIN_PUSH, arg0: *pattern, arg1: 0.0 }
        }
        ResolvedActionKind::ChainClear { target_id } => {
            NodeCommand { target_id: *target_id, type_id: CMD_CHAIN_CLEAR, arg0: 0, arg1: 0.0 }
        }
    };
    conf.send_command(cmd).map_err(|_| "command ring buffer full".into())
}

fn param_id_for_name(name: &str) -> u32 {
    ParamDescriptor::id_for_name(name)
}

/// Shared engine stack for both batch and interactive modes: the built graph and
/// its executor behind a `Mutex`, a spawned null-backend audio thread writing
/// into the lock-free capture ring, and the name resolver. The audio thread runs
/// until `TestContext::running` is cleared. (ADR-033 § null audio backend.)
fn build_context(
    def: &paraclete_app::instrument::InstrumentDefinition,
    sample_rate: f32,
    block_size: usize,
) -> Result<TestContext, String> {
    let resolver = NameResolver::from_instrument(def);

    let mut conf = NodeConfigurator::new(sample_rate, block_size);
    let libraries = HashMap::new();
    let _ids = build_from_instrument(def, &mut conf, &libraries)
        .map_err(|e| format!("failed to build graph: {}", e))?;

    let bus_handle = conf.state_bus_handle();
    let executor = Arc::new(Mutex::new(conf.build_executor()));
    executor.lock().unwrap().set_debug_log_enabled(true);
    let capture = Arc::new(CaptureRing::new(CAPTURE_RING_CAPACITY));
    let running = Arc::new(AtomicBool::new(true));

    let cap = capture.clone();
    let exec = executor.clone();
    let run = running.clone();
    let channels = 2usize;
    std::thread::spawn(move || {
        let mut block = vec![0.0f32; block_size * channels];
        while run.load(Ordering::SeqCst) {
            let mut ex = exec.lock().unwrap();
            ex.process(&mut block, channels);
            drop(ex);
            cap.push(block.chunks(channels).map(|ch| ch[0]).collect());
            let sleep_us = (block_size as f64 / sample_rate as f64 * 1_000_000.0) as u64;
            if sleep_us > 0 {
                std::thread::sleep(Duration::from_micros(sleep_us));
            }
        }
    });

    Ok(TestContext {
        conf, executor, bus_handle, capture, running,
        resolver, sample_rate, block_size,
    })
}

/// Regression-baseline action (ADR-035 Part A). The `String` is the resolved
/// `<scenario>.baseline.json` path.
enum BaselineMode {
    Off,
    Update(String),
    Check(String),
}

fn run_batch(scenario: TestScenario) -> Result<(), String> {
    let instrument_path = PathBuf::from(&scenario.instrument);
    let def = load_instrument_definition(&instrument_path)
        .map_err(|e| format!("failed to load instrument: {}", e))?;

    let sample_rate = scenario.sample_rate;
    let block_size = scenario.block_size;

    let mut ctx = build_context(&def, sample_rate, block_size)?;

    let mut timeline: Vec<(f64, ResolvedActionKind)> = scenario.timeline.iter().map(|entry| {
        let kind = resolve_action(&ctx.resolver, &entry.action)?;
        Ok((entry.at, kind))
    }).collect::<Result<Vec<_>, String>>()?;
    timeline.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let start = Instant::now();
    let duration = Duration::from_secs_f64(scenario.duration_secs);
    let mut next_action = 0usize;
    let mut last_capture_read: usize = 0;
    let total_samples = (sample_rate as f64 * scenario.duration_secs) as usize;
    let mut all_samples: Vec<f32> = Vec::with_capacity(total_samples);
    let mut failures: Vec<String> = Vec::new();
    let mut probes_to_check: Vec<&Probe> = scenario.probe.iter().collect();
    let mut assertions_to_check: Vec<&Assertion> = scenario.assert.iter().collect();

    while start.elapsed() < duration {
        std::thread::sleep(Duration::from_millis(1));
        ctx.conf.process_main_thread();

        let elapsed = start.elapsed().as_secs_f64();

        while next_action < timeline.len() && timeline[next_action].0 <= elapsed {
            let action = &timeline[next_action];
            dispatch_action(&mut ctx.conf, &action.1)?;
            next_action += 1;
        }

        ctx.capture.drain(&mut all_samples, &mut last_capture_read);

        probes_to_check.retain(|p| {
            if elapsed >= p.at {
                if let Some(val) = ctx.conf.state_bus_read(&p.path) {
                    eprintln!("[probe] {}s: {} = {:?}", p.at, p.path, val);
                } else {
                    eprintln!("[probe] {}s: {} = <no value>", p.at, p.path);
                }
                false
            } else {
                true
            }
        });

        assertions_to_check.retain(|a| {
            if elapsed >= a.at {
                if let Some(path) = &a.path {
                    let val = ctx.conf.state_bus_read(path);
                    if let Some(eq) = a.eq {
                        match val {
                            Some(StateBusValue::Float(v)) if (v - eq).abs() < 1e-6 => {},
                            Some(StateBusValue::Int(v)) if (v as f64 - eq).abs() < 1e-6 => {},
                            _ => failures.push(format!(
                                "assertion at {}s: {} expected {}, got {:?}",
                                a.at, path, eq, val
                            )),
                        }
                    }
                    if let Some(between) = &a.between {
                        match val {
                            Some(StateBusValue::Float(v)) if v >= between[0] && v <= between[1] => {},
                            Some(StateBusValue::Int(v)) if (v as f64) >= between[0] && (v as f64) <= between[1] => {},
                            _ => failures.push(format!(
                                "assertion at {}s: {} expected between {:?}, got {:?}",
                                a.at, path, between, val
                            )),
                        }
                    }
                }
                if a.peak_gte.is_some() || a.peak_lt.is_some() {
                    let window_ms = a.window_ms.unwrap_or(500.0);
                    let window_samples = (window_ms / 1000.0 * sample_rate as f64) as usize;
                    let total = all_samples.len();
                    if total > window_samples {
                        let mut peak = 0.0f32;
                        for s in &all_samples[total - window_samples..] {
                            peak = peak.max(s.abs());
                        }
                        if let Some(min_peak) = a.peak_gte {
                            if peak < min_peak as f32 {
                                failures.push(format!(
                                    "assertion at {}s: peak {:.4} < {:.4}",
                                    a.at, peak, min_peak
                                ));
                            }
                        }
                        if let Some(max_peak) = a.peak_lt {
                            if peak >= max_peak as f32 {
                                failures.push(format!(
                                    "assertion at {}s: peak {:.4} >= {:.4}",
                                    a.at, peak, max_peak
                                ));
                            }
                        }
                    }
                }
                false
            } else {
                true
            }
        });
    }

    ctx.capture.drain(&mut all_samples, &mut last_capture_read);
    ctx.running.store(false, Ordering::SeqCst);

    check_artifact_assertions(&scenario.assert, &all_samples, sample_rate, &mut failures);

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("[test-driver] FAIL: {}", f);
        }
        eprintln!("[test-driver] {} assertion(s) failed", failures.len());
        return Err("assertions failed".into());
    }

    wav::write_wav(&scenario.output, &all_samples, sample_rate as u32)
        .map_err(|e| format!("failed to write WAV: {}", e))?;
    eprintln!("[test-driver] wrote {} ({} samples, {:.1}s)",
        scenario.output, all_samples.len(),
        all_samples.len() as f64 / sample_rate as f64);

    let debug_events = ctx.conf.debug_events();
    if !debug_events.is_empty() {
        eprintln!("[test-driver] debug events ({}):", debug_events.len());
        for ev in &debug_events {
            eprintln!("  node={} kind={} sample={} arg0={} arg1={}",
                ev.node_id, ev.kind.as_str(), ev.sample_offset, ev.arg0, ev.arg1);
        }
    }

    if scenario.play {
        let output = scenario.output.clone();
        let player = auto_play_command();
        let status = std::process::Command::new(player)
            .arg(&output)
            .status();
        if let Err(e) = status {
            eprintln!("[test-driver] {} failed: {} (output at {})", player, e, output);
        }
    }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Interactive mode (ADR-033 § interactive) — a JSON-lines REPL over the same
// engine stack as batch mode. A dedicated reader thread keeps stdin off the main
// loop so the state-bus drain and audio capture never stall (hostile-review
// issue #1). Deviation from the ADR: an unbounded `std::sync::mpsc` channel
// replaces the bounded `rtrb` SPSC — the core requirement (the main thread never
// blocks on stdin) holds, and an unbounded queue loses no commands under a burst
// rather than dropping the oldest.
// ─────────────────────────────────────────────────────────────────────────────

struct InteractiveConfig {
    instrument: String,
    sample_rate: f32,
    block_size: usize,
}

fn parse_interactive_args(args: &[String]) -> Result<InteractiveConfig, String> {
    let mut cfg = InteractiveConfig {
        instrument: "instrument.yaml".to_string(),
        sample_rate: 44100.0,
        block_size: 512,
    };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--instrument" => {
                i += 1;
                cfg.instrument = args.get(i).ok_or("--instrument needs a value")?.clone();
            }
            "--sample-rate" => {
                i += 1;
                cfg.sample_rate = args.get(i).ok_or("--sample-rate needs a value")?
                    .parse().map_err(|_| "--sample-rate must be a number".to_string())?;
            }
            "--block-size" => {
                i += 1;
                cfg.block_size = args.get(i).ok_or("--block-size needs a value")?
                    .parse().map_err(|_| "--block-size must be an integer".to_string())?;
            }
            other => return Err(format!("unknown interactive flag: {}", other)),
        }
        i += 1;
    }
    if !cfg.sample_rate.is_finite() || cfg.sample_rate <= 0.0 {
        return Err("--sample-rate must be positive".into());
    }
    if cfg.block_size == 0 {
        return Err("--block-size must be > 0".into());
    }
    Ok(cfg)
}

fn run_interactive(cfg: &InteractiveConfig) -> Result<(), String> {
    use std::io::Write;

    let def = load_instrument_definition(&PathBuf::from(&cfg.instrument))
        .map_err(|e| format!("failed to load instrument: {}", e))?;
    let mut ctx = build_context(&def, cfg.sample_rate, cfg.block_size)?;

    // Reader thread: blocks on stdin, forwards each line to the main loop. It
    // never touches the engine, so a blocked read cannot stall audio or state.
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) => break, // EOF → drop tx → main loop exits
                Ok(_) => {
                    if tx.send(line.clone()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    eprintln!("[test-driver] interactive mode ready — JSON commands on stdin, responses on stdout");

    let mut all_samples: Vec<f32> = Vec::new();
    let mut last_capture_read: usize = 0;
    let stdout = std::io::stdout();

    loop {
        std::thread::sleep(Duration::from_millis(1));
        ctx.conf.process_main_thread();
        ctx.capture.drain(&mut all_samples, &mut last_capture_read);

        match rx.try_recv() {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let (resp, quit) = handle_json_command(&mut ctx, trimmed, &all_samples);
                let mut out = stdout.lock();
                let _ = writeln!(out, "{}", resp);
                let _ = out.flush();
                if quit {
                    break;
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
        }
    }

    ctx.running.store(false, Ordering::SeqCst);
    Ok(())
}

fn ok_json() -> String {
    "{\"ok\":true}".to_string()
}

fn err_json(msg: &str) -> String {
    serde_json::json!({ "error": msg }).to_string()
}

fn value_to_json(v: &StateBusValue) -> serde_json::Value {
    match v {
        StateBusValue::Float(f) => serde_json::json!(f),
        StateBusValue::Int(i) => serde_json::json!(i),
        StateBusValue::Bool(b) => serde_json::json!(b),
        StateBusValue::Text(s) => serde_json::json!(s),
    }
}

fn jstr<'a>(v: &'a serde_json::Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(|x| x.as_str())
}

fn need_f64(v: &serde_json::Value, k: &str, cmd: &str) -> Result<f64, String> {
    v.get(k).and_then(|x| x.as_f64()).ok_or_else(|| format!("{} needs numeric '{}'", cmd, k))
}

fn need_i64(v: &serde_json::Value, k: &str, cmd: &str) -> Result<i64, String> {
    v.get(k).and_then(|x| x.as_i64()).ok_or_else(|| format!("{} needs integer '{}'", cmd, k))
}

fn resolve_json_target(resolver: &NameResolver, v: &serde_json::Value) -> Result<u32, String> {
    let t = v.get("target").ok_or_else(|| "missing 'target'".to_string())?;
    if let Some(n) = t.as_i64() {
        resolver.resolve_required(&n.to_string())
    } else if let Some(s) = t.as_str() {
        resolver.resolve_required(s)
    } else {
        Err("'target' must be a node id or name".into())
    }
}

/// Map an interactive command name + its JSON fields to a `ResolvedActionKind`.
/// `Ok(None)` means the command is not an engine mutation (caller handles the
/// interactive-only `read`/`dump`/`peak`/`render`/`quit` verbs).
fn json_to_action(
    resolver: &NameResolver,
    cmd: &str,
    v: &serde_json::Value,
) -> Result<Option<ResolvedActionKind>, String> {
    use ResolvedActionKind as A;
    let action = match cmd {
        "set_param" => A::SetParam {
            target_id: resolve_json_target(resolver, v)?,
            param_name: jstr(v, "param").ok_or("set_param needs 'param'")?.to_string(),
            value: need_f64(v, "value", cmd)?,
        },
        "bump_param" => A::BumpParam {
            target_id: resolve_json_target(resolver, v)?,
            param_name: jstr(v, "param").ok_or("bump_param needs 'param'")?.to_string(),
            delta: need_f64(v, "delta", cmd)?,
        },
        "trigger" => A::Trigger {
            target_id: resolve_json_target(resolver, v)?,
            note: v.get("note").and_then(|x| x.as_i64()).unwrap_or(-1),
            velocity: v.get("velocity").and_then(|x| x.as_f64()).unwrap_or(0.79),
        },
        "toggle_step" => A::ToggleStep {
            target_id: resolve_json_target(resolver, v)?,
            step: need_i64(v, "step", cmd)?,
        },
        "set_step" => A::SetStep {
            target_id: resolve_json_target(resolver, v)?,
            step: need_i64(v, "step", cmd)?,
            note: need_i64(v, "note", cmd)?,
        },
        "clear" => A::Clear { target_id: resolve_json_target(resolver, v)? },
        "set_pattern" => A::SetPattern {
            target_id: resolve_json_target(resolver, v)?,
            pattern: need_i64(v, "pattern", cmd)?,
        },
        "set_length" => A::SetLength {
            target_id: resolve_json_target(resolver, v)?,
            steps: need_i64(v, "steps", cmd)?,
        },
        "set_speed" => A::SetSpeed {
            target_id: resolve_json_target(resolver, v)?,
            speed: need_f64(v, "speed", cmd)?,
        },
        "set_page_loop" => A::SetPageLoop {
            target_id: resolve_json_target(resolver, v)?,
            start_page: need_i64(v, "start_page", cmd)?,
            end_page: need_i64(v, "end_page", cmd)?,
        },
        "set_step_timing" => A::SetStepTiming {
            target_id: resolve_json_target(resolver, v)?,
            step: need_i64(v, "step", cmd)?,
            micro_offset: need_i64(v, "micro_offset", cmd)?,
        },
        "set_fill_a" => A::SetFillA {
            target_id: resolve_json_target(resolver, v)?,
            active: v.get("active").and_then(|x| x.as_bool()).ok_or("set_fill_a needs bool 'active'")?,
        },
        "set_fill_b" => A::SetFillB {
            target_id: resolve_json_target(resolver, v)?,
            active: v.get("active").and_then(|x| x.as_bool()).ok_or("set_fill_b needs bool 'active'")?,
        },
        "set_step_condition" => A::SetStepCondition {
            target_id: resolve_json_target(resolver, v)?,
            step: need_i64(v, "step", cmd)?,
            probability: need_i64(v, "probability", cmd)? as u8,
            repeat_n: need_i64(v, "repeat_n", cmd)? as u8,
            repeat_m: need_i64(v, "repeat_m", cmd)? as u8,
            fill: need_i64(v, "fill", cmd)? as u8,
        },
        "chain_push" => A::ChainPush {
            target_id: resolve_json_target(resolver, v)?,
            pattern: need_i64(v, "pattern", cmd)?,
        },
        "chain_clear" => A::ChainClear { target_id: resolve_json_target(resolver, v)? },
        _ => return Ok(None),
    };
    Ok(Some(action))
}

/// Dispatch one parsed interactive command. Returns the JSON response line and
/// whether the session should quit. Engine mutations reuse the batch
/// `dispatch_action` path; `read`/`dump`/`peak`/`render` are interactive-only.
fn handle_json_command(ctx: &mut TestContext, line: &str, all_samples: &[f32]) -> (String, bool) {
    let v: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return (err_json(&format!("invalid JSON: {}", e)), false),
    };
    let cmd = match v.get("cmd").and_then(|c| c.as_str()) {
        Some(c) => c,
        None => return (err_json("missing 'cmd' field"), false),
    };

    match cmd {
        "quit" => (ok_json(), true),

        "read" => match jstr(&v, "path") {
            None => (err_json("read needs a 'path'"), false),
            Some(path) => match ctx.conf.state_bus_read(path) {
                Some(val) => (
                    serde_json::json!({ "path": path, "value": value_to_json(&val) }).to_string(),
                    false,
                ),
                None => (err_json(&format!("no value at path {}", path)), false),
            },
        },

        "dump" => {
            let bus = ctx.bus_handle.borrow();
            let mut paths = serde_json::Map::new();
            for (path, val) in bus.iter() {
                paths.insert(path.to_string(), value_to_json(val));
            }
            (serde_json::json!({ "paths": paths }).to_string(), false)
        }

        "peak" => {
            let window_ms = v.get("window_ms").and_then(|w| w.as_f64()).unwrap_or(500.0);
            let window_samples = (window_ms / 1000.0 * ctx.sample_rate as f64) as usize;
            let start = all_samples.len().saturating_sub(window_samples);
            let peak = all_samples[start..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            (serde_json::json!({ "peak": peak, "window_ms": window_ms }).to_string(), false)
        }

        "render" => {
            let output = jstr(&v, "output").unwrap_or("/tmp/paraclete_debug.wav");
            match wav::write_wav(output, all_samples, ctx.sample_rate as u32) {
                Ok(()) => (
                    serde_json::json!({ "ok": true, "output": output, "samples": all_samples.len() }).to_string(),
                    false,
                ),
                Err(e) => (err_json(&format!("render failed: {}", e)), false),
            }
        }

        "log" => {
            // Drain any events already in the SPSC, then wait one audio-block
            // for the executor to process pending commands (the null backend
            // sleeps block_size/sample_rate between process() calls).
            ctx.conf.process_main_thread();
            std::thread::sleep(std::time::Duration::from_micros(
                (ctx.block_size as f64 / ctx.sample_rate as f64 * 1_000_000.0) as u64 + 1000,
            ));
            ctx.conf.process_main_thread();
            let events = ctx.conf.debug_events();
            let json_events: Vec<serde_json::Value> = events.iter().map(|ev| {
                serde_json::json!({
                    "t": ev.sample_offset,
                    "node": ev.node_id,
                    "kind": ev.kind.as_str(),
                    "arg0": ev.arg0,
                    "arg1": ev.arg1,
                })
            }).collect();
            (serde_json::json!({ "events": json_events }).to_string(), false)
        }

        other => match json_to_action(&ctx.resolver, other, &v) {
            Ok(Some(action)) => match dispatch_action(&mut ctx.conf, &action) {
                Ok(()) => (ok_json(), false),
                Err(e) => (err_json(&e), false),
            },
            Ok(None) => (err_json(&format!("unknown command: {}", other)), false),
            Err(e) => (err_json(&e), false),
        },
    }
}

/// Post-capture artifact scans (INFRA-001). Unlike live assertions these run
/// on the complete buffer once the render finishes; `from`/`until` bound the
/// scanned window in seconds.
fn check_artifact_assertions(
    assertions: &[Assertion],
    all_samples: &[f32],
    sample_rate: f32,
    failures: &mut Vec<String>,
) {
    for a in assertions {
        if !a.has_artifact_check() {
            continue;
        }
        let from = ((a.from.unwrap_or(0.0) * sample_rate as f64) as usize).min(all_samples.len());
        let until = a.until
            .map(|u| ((u * sample_rate as f64) as usize).min(all_samples.len()))
            .unwrap_or(all_samples.len());
        if from >= until {
            failures.push(format!(
                "artifact assertion window [{}s, {}s) is empty (capture is {:.3}s)",
                a.from.unwrap_or(0.0),
                a.until.map(|u| u.to_string()).unwrap_or_else(|| "end".into()),
                all_samples.len() as f64 / sample_rate as f64
            ));
            continue;
        }
        let window = &all_samples[from..until];
        let time_of = |idx: usize| (from + idx) as f64 / sample_rate as f64;

        // NaN defeats the ordered comparisons below (NaN >= limit is
        // false), so any non-finite sample fails the assertion outright.
        let (nf_count, nf_idx) = analysis::non_finite(window);
        if nf_count > 0 {
            failures.push(format!(
                "{} non-finite sample(s), first at sample {} ({:.4}s)",
                nf_count, from + nf_idx, time_of(nf_idx)
            ));
            continue;
        }

        if let Some(limit) = a.discontinuity_lt {
            let (jump, idx) = analysis::max_discontinuity(window);
            if jump as f64 >= limit {
                failures.push(format!(
                    "discontinuity {:.4} at sample {} ({:.4}s) >= {:.4}",
                    jump, from + idx, time_of(idx), limit
                ));
            }
        }
        if let Some(limit) = a.dc_offset_lt {
            let offset = analysis::dc_offset(window);
            if offset.abs() as f64 >= limit {
                failures.push(format!(
                    "dc offset {:.5} over [{:.3}s, {:.3}s) >= {:.5}",
                    offset, time_of(0), until as f64 / sample_rate as f64, limit
                ));
            }
        }
        if let Some(limit_ms) = a.dropout_lt_ms {
            let (run, idx) = analysis::longest_hold_run(window);
            let run_ms = run as f64 / sample_rate as f64 * 1000.0;
            if run_ms >= limit_ms {
                failures.push(format!(
                    "held-sample run of {:.2}ms ({} samples) starting at {:.4}s >= {:.2}ms",
                    run_ms, run, time_of(idx), limit_ms
                ));
            }
        }
    }
}

fn resolve_action(resolver: &NameResolver, action: &scenario::TimelineAction) -> Result<ResolvedActionKind, String> {
    use scenario::TimelineAction;
    Ok(match action {
        TimelineAction::SetParam { target, param, value } => ResolvedActionKind::SetParam {
            target_id: resolve_target(resolver, target)?, param_name: param.clone(), value: *value,
        },
        TimelineAction::BumpParam { target, param, delta } => ResolvedActionKind::BumpParam {
            target_id: resolve_target(resolver, target)?, param_name: param.clone(), delta: *delta,
        },
        TimelineAction::Trigger { target, note, velocity } => ResolvedActionKind::Trigger {
            target_id: resolve_target(resolver, target)?, note: *note, velocity: *velocity,
        },
        TimelineAction::ToggleStep { target, step } => ResolvedActionKind::ToggleStep {
            target_id: resolve_target(resolver, target)?, step: *step,
        },
        TimelineAction::SetStep { target, step, note } => ResolvedActionKind::SetStep {
            target_id: resolve_target(resolver, target)?, step: *step, note: *note,
        },
        TimelineAction::Clear { target } => ResolvedActionKind::Clear {
            target_id: resolve_target(resolver, target)?,
        },
        TimelineAction::SetPattern { target, pattern } => ResolvedActionKind::SetPattern {
            target_id: resolve_target(resolver, target)?, pattern: *pattern,
        },
        TimelineAction::SetLength { target, steps } => ResolvedActionKind::SetLength {
            target_id: resolve_target(resolver, target)?, steps: *steps,
        },
        TimelineAction::SetSpeed { target, speed } => ResolvedActionKind::SetSpeed {
            target_id: resolve_target(resolver, target)?, speed: *speed,
        },
        TimelineAction::SetPageLoop { target, start_page, end_page } => ResolvedActionKind::SetPageLoop {
            target_id: resolve_target(resolver, target)?, start_page: *start_page, end_page: *end_page,
        },
        TimelineAction::SetStepTiming { target, step, micro_offset } => ResolvedActionKind::SetStepTiming {
            target_id: resolve_target(resolver, target)?, step: *step, micro_offset: *micro_offset,
        },
        TimelineAction::SetFillA { target, active } => ResolvedActionKind::SetFillA {
            target_id: resolve_target(resolver, target)?, active: *active,
        },
        TimelineAction::SetFillB { target, active } => ResolvedActionKind::SetFillB {
            target_id: resolve_target(resolver, target)?, active: *active,
        },
        TimelineAction::SetStepCondition { target, step, probability, repeat_n, repeat_m, fill } =>
            ResolvedActionKind::SetStepCondition {
                target_id: resolve_target(resolver, target)?, step: *step,
                probability: *probability, repeat_n: *repeat_n, repeat_m: *repeat_m, fill: *fill,
            },
        TimelineAction::ChainPush { target, pattern } => ResolvedActionKind::ChainPush {
            target_id: resolve_target(resolver, target)?, pattern: *pattern,
        },
        TimelineAction::ChainClear { target } => ResolvedActionKind::ChainClear {
            target_id: resolve_target(resolver, target)?,
        },
    })
}

const QUICK_USAGE: &str = "usage: test-driver <test.yaml> [--update-baseline | --check-baseline]
       test-driver --trigger <target> --at <secs> [--trigger T --at S ...]
                   [-d <secs>] [--instrument <path>] [--output <path>] [--no-play]
       test-driver --interactive [--instrument <path>] [--sample-rate <hz>] [--block-size <n>]";

/// Truncate/zero-pad the captured buffer to exactly `duration_secs × sample_rate`
/// samples. Wall-clock-bounded capture jitters by a block or two run-to-run
/// (AGENTS.md); a fixed length makes the baseline fingerprint's sample_count and
/// envelope-window count deterministic. Trailing samples are the decayed tail;
/// a short render pads with silence (deterministic, negligible energy).
fn fixed_len(samples: &[f32], sample_rate: f32, duration_secs: f64) -> Vec<f32> {
    let target = (sample_rate as f64 * duration_secs).round() as usize;
    let mut v = Vec::with_capacity(target);
    v.extend_from_slice(&samples[..samples.len().min(target)]);
    v.resize(target, 0.0);
    v
}

/// `<scenario>.yaml` → `<scenario>.baseline.json` beside it.
fn baseline_path_for(scenario_yaml: &str) -> String {
    let stem = scenario_yaml
        .strip_suffix(".yaml")
        .or_else(|| scenario_yaml.strip_suffix(".yml"))
        .unwrap_or(scenario_yaml);
    format!("{}.baseline.json", stem)
}

/// Parse the optional trailing baseline flag after a scenario file. Unknown
/// trailing args are an error (a typo shouldn't silently run a plain render).
fn parse_baseline_flag(args: &[String], yaml_path: &str) -> Result<BaselineMode, String> {
    let mut mode = BaselineMode::Off;
    for arg in args {
        match arg.as_str() {
            "--update-baseline" => mode = BaselineMode::Update(baseline_path_for(yaml_path)),
            "--check-baseline" => mode = BaselineMode::Check(baseline_path_for(yaml_path)),
            other => return Err(format!("unknown flag after scenario: {}", other)),
        }
    }
    Ok(mode)
}

/// Deterministic, single-threaded render for baseline runs (ADR-035 Part A). No
/// audio thread and no wall clock: step the executor block-by-block, applying
/// each timeline action at its sample-accurate block boundary. The result is
/// bit-identical run to run, so an envelope fingerprint is meaningful — the
/// threaded `run_batch` path jitters by a block (a trigger lands in different
/// blocks each run), which makes per-window comparison unstable.
fn render_deterministic(scenario: &TestScenario) -> Result<Vec<f32>, String> {
    let def = load_instrument_definition(&PathBuf::from(&scenario.instrument))
        .map_err(|e| format!("failed to load instrument: {}", e))?;
    let sample_rate = scenario.sample_rate;
    let block_size = scenario.block_size;
    let resolver = NameResolver::from_instrument(&def);

    let mut conf = NodeConfigurator::new(sample_rate, block_size);
    let libraries = HashMap::new();
    build_from_instrument(&def, &mut conf, &libraries)
        .map_err(|e| format!("failed to build graph: {}", e))?;
    let mut executor = conf.build_executor();
    executor.set_debug_log_enabled(true);

    let mut timeline: Vec<(usize, ResolvedActionKind)> = scenario
        .timeline
        .iter()
        .map(|entry| {
            let kind = resolve_action(&resolver, &entry.action)?;
            let at = (entry.at.max(0.0) * sample_rate as f64).round() as usize;
            Ok((at, kind))
        })
        .collect::<Result<Vec<_>, String>>()?;
    timeline.sort_by_key(|(at, _)| *at);

    let channels = 2usize;
    let target = (sample_rate as f64 * scenario.duration_secs).round() as usize;
    let n_blocks = target.div_ceil(block_size);
    let mut all = Vec::with_capacity(n_blocks * block_size);
    let mut block = vec![0.0f32; block_size * channels];
    let mut next = 0usize;

    for b in 0..n_blocks {
        let block_end = (b + 1) * block_size;
        // Commands sent before process() are drained and applied by the executor
        // in that same call, so an action fires in the block its offset lands in.
        while next < timeline.len() && timeline[next].0 < block_end {
            dispatch_action(&mut conf, &timeline[next].1)?;
            next += 1;
        }
        block.iter_mut().for_each(|s| *s = 0.0);
        executor.process(&mut block, channels);
        all.extend(block.chunks(channels).map(|ch| ch[0]));
        conf.process_main_thread();
    }
    Ok(all)
}

/// Baseline update/check over a deterministic render (ADR-035 Part A). Update
/// writes only over a clean render (never bake a failing render into a
/// baseline); check folds drift into the same failure path as assertions, so a
/// regression exits 1 like any other failed check.
fn run_baseline(scenario: TestScenario, mode: BaselineMode) -> Result<(), String> {
    let sample_rate = scenario.sample_rate;
    let block_size = scenario.block_size;

    let all_samples = render_deterministic(&scenario)?;
    let mut failures: Vec<String> = Vec::new();

    // The baseline fingerprint IS the regression check — we don't re-run the
    // scenario's artifact assertions here (they are tuned for the threaded
    // render; the deterministic render legitimately differs, e.g. exact-zero
    // silence). The only render-sanity guard is non-finite samples.
    let fp = baseline::compute_fingerprint(
        &fixed_len(&all_samples, sample_rate, scenario.duration_secs),
        sample_rate,
    );

    match &mode {
        BaselineMode::Off => {}
        BaselineMode::Update(path) => {
            if fp.non_finite > 0 {
                eprintln!("[test-driver] NOT updating baseline — {} non-finite sample(s) in render", fp.non_finite);
            } else {
                // Preserve hand-tuned tolerances across an update; refresh only
                // the fingerprint.
                let tolerances = baseline::load(path).map(|b| b.tolerances).unwrap_or_default();
                baseline::save(path, &baseline::Baseline { fingerprint: fp, tolerances })?;
                eprintln!("[test-driver] wrote baseline {}", path);
            }
        }
        BaselineMode::Check(path) => match baseline::load(path) {
            Ok(b) => {
                for d in baseline::compare(&b, &fp, block_size) {
                    failures.push(format!("baseline drift: {}", d));
                }
            }
            Err(e) => failures.push(e),
        },
    }

    if !failures.is_empty() {
        for f in &failures {
            eprintln!("[test-driver] FAIL: {}", f);
        }
        eprintln!("[test-driver] {} check(s) failed", failures.len());
        return Err("assertions failed".into());
    }
    eprintln!("[test-driver] baseline OK ({} samples)", all_samples.len());
    Ok(())
}

/// Quick mode (INFRA-002 / ADR-033 § Quick mode): build a scenario from
/// one-liner flags. `--trigger`/`--at` pair positionally; mismatched counts
/// error before execution. Duration defaults to the last trigger + 2s.
fn parse_quick_args(args: &[String]) -> Result<TestScenario, String> {
    fn value<'a>(args: &'a [String], i: &mut usize, flag: &str) -> Result<&'a str, String> {
        *i += 1;
        match args.get(*i).map(|s| s.as_str()) {
            // A flag-like token means the value is missing — without this,
            // `--output --no-play` silently sets output to "--no-play".
            Some(v) if v.starts_with("--") => Err(format!("{} needs a value, got flag '{}'", flag, v)),
            Some(v) => Ok(v),
            None => Err(format!("{} needs a value", flag)),
        }
    }
    fn number(args: &[String], i: &mut usize, flag: &str) -> Result<f64, String> {
        let v = value(args, i, flag)?;
        v.parse().map_err(|_| format!("{} needs a number, got '{}'", flag, v))
    }

    let mut triggers: Vec<String> = Vec::new();
    let mut ats: Vec<f64> = Vec::new();
    let mut duration: Option<f64> = None;
    let mut instrument = "instrument.yaml".to_string();
    let mut output = "/tmp/paraclete_test.wav".to_string();
    let mut play = true;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trigger" => triggers.push(value(args, &mut i, "--trigger")?.to_string()),
            "--at" => ats.push(number(args, &mut i, "--at")?),
            d @ ("-d" | "--duration") => duration = Some(number(args, &mut i, d)?),
            "--instrument" => instrument = value(args, &mut i, "--instrument")?.to_string(),
            "--output" => output = value(args, &mut i, "--output")?.to_string(),
            "--no-play" => play = false,
            other => return Err(format!("unknown argument: {}\n{}", other, QUICK_USAGE)),
        }
        i += 1;
    }

    if triggers.len() != ats.len() {
        return Err(format!(
            "{} --trigger value(s) but {} --at value(s) — counts must match",
            triggers.len(), ats.len()
        ));
    }
    if triggers.is_empty() {
        return Err(format!("quick mode needs at least one --trigger/--at pair\n{}", QUICK_USAGE));
    }

    if ats.iter().any(|a| !a.is_finite() || *a < 0.0) {
        return Err("--at values must be finite and >= 0".into());
    }
    let last_at = ats.iter().cloned().fold(0.0f64, f64::max);
    let duration_secs = duration.unwrap_or(last_at + 2.0);
    // run_batch feeds this to Duration::from_secs_f64, which panics on
    // negative/NaN — reject here for a clean CLI error instead.
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Err(format!("duration must be positive and finite, got {}", duration_secs));
    }

    let timeline = triggers.into_iter().zip(ats).map(|(target, at)| {
        scenario::TimelineEntry {
            at,
            action: scenario::TimelineAction::Trigger { target, note: -1, velocity: 0.79 },
        }
    }).collect();

    Ok(TestScenario {
        format_version: 1,
        instrument,
        sample_rate: 44100.0,
        block_size: 512,
        duration_secs,
        output,
        play,
        timeline,
        assert: Vec::new(),
        probe: Vec::new(),
    })
}

#[cfg(test)]
mod quick_mode_tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn one_liner_builds_scenario() {
        let s = parse_quick_args(&args("--trigger kick --at 1.0 -d 3")).unwrap();
        assert_eq!(s.duration_secs, 3.0);
        assert_eq!(s.timeline.len(), 1);
        assert_eq!(s.timeline[0].at, 1.0);
        match &s.timeline[0].action {
            scenario::TimelineAction::Trigger { target, note, .. } => {
                assert_eq!(target, "kick");
                assert_eq!(*note, -1, "quick-mode trigger must use engine default note");
            }
            other => panic!("expected trigger, got {:?}", other),
        }
    }

    #[test]
    fn triggers_and_ats_pair_positionally() {
        let s = parse_quick_args(&args("--trigger kick --at 1.0 --trigger snare --at 1.5")).unwrap();
        assert_eq!(s.timeline.len(), 2);
        assert_eq!(s.timeline[1].at, 1.5);
        // duration defaults to last trigger + 2s
        assert_eq!(s.duration_secs, 3.5);
    }

    #[test]
    fn mismatched_counts_error_before_execution() {
        let err = parse_quick_args(&args("--trigger kick --trigger snare --at 1.0")).unwrap_err();
        assert!(err.contains("counts must match"), "got: {}", err);
    }

    #[test]
    fn no_triggers_is_an_error() {
        assert!(parse_quick_args(&args("-d 3")).is_err());
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let err = parse_quick_args(&args("--trigger kick --at 1.0 --bogus")).unwrap_err();
        assert!(err.contains("unknown argument: --bogus"), "got: {}", err);
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(parse_quick_args(&args("--trigger kick --at")).is_err());
    }

    #[test]
    fn no_play_and_output_are_respected() {
        let s = parse_quick_args(&args("--trigger kick --at 0.5 --no-play --output /tmp/q.wav")).unwrap();
        assert!(!s.play);
        assert_eq!(s.output, "/tmp/q.wav");
    }

    #[test]
    fn flag_like_value_is_an_error_not_a_silent_swallow() {
        let err = parse_quick_args(&args("--trigger kick --at 0.5 --output --no-play")).unwrap_err();
        assert!(err.contains("--output needs a value"), "got: {}", err);
    }

    #[test]
    fn negative_or_nan_duration_is_a_clean_error() {
        assert!(parse_quick_args(&args("--trigger kick --at 0.5 -d -5")).is_err());
        assert!(parse_quick_args(&args("--trigger kick --at 0.5 -d nan")).is_err());
    }

    #[test]
    fn negative_at_is_an_error() {
        assert!(parse_quick_args(&args("--trigger kick --at -1.0")).is_err());
    }
}

#[cfg(test)]
mod interactive_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn value_to_json_covers_all_variants() {
        assert_eq!(value_to_json(&StateBusValue::Float(1.5)), json!(1.5));
        assert_eq!(value_to_json(&StateBusValue::Int(3)), json!(3));
        assert_eq!(value_to_json(&StateBusValue::Bool(true)), json!(true));
        assert_eq!(value_to_json(&StateBusValue::Text("x".into())), json!("x"));
    }

    #[test]
    fn need_helpers_enforce_presence_and_type() {
        let v = json!({ "value": 0.3, "step": 4 });
        assert_eq!(need_f64(&v, "value", "set_param").unwrap(), 0.3);
        assert!(need_f64(&v, "missing", "set_param").is_err());
        assert_eq!(need_i64(&v, "step", "toggle_step").unwrap(), 4);
        // a float is not an integer
        assert!(need_i64(&v, "value", "toggle_step").is_err());
    }

    #[test]
    fn unknown_command_is_none_not_error() {
        let r = NameResolver::empty();
        assert!(matches!(json_to_action(&r, "frobnicate", &json!({})), Ok(None)));
    }

    #[test]
    fn set_param_parses_with_numeric_target() {
        let r = NameResolver::empty();
        let action = json_to_action(&r, "set_param",
            &json!({ "target": 20, "param": "decay", "value": 0.3 })).unwrap().unwrap();
        match action {
            ResolvedActionKind::SetParam { target_id, param_name, value } => {
                assert_eq!(target_id, 20);
                assert_eq!(param_name, "decay");
                assert_eq!(value, 0.3);
            }
            other => panic!("expected SetParam, got {:?}", other),
        }
    }

    #[test]
    fn set_param_missing_field_is_error() {
        let r = NameResolver::empty();
        assert!(json_to_action(&r, "set_param", &json!({ "target": 20, "value": 0.3 })).is_err());
    }

    #[test]
    fn trigger_defaults_note_and_velocity() {
        let r = NameResolver::empty();
        let action = json_to_action(&r, "trigger", &json!({ "target": 20 })).unwrap().unwrap();
        match action {
            // ADR-033: note < 0 = engine default (not 0, which would retune — BUG-028)
            ResolvedActionKind::Trigger { note, velocity, .. } => {
                assert_eq!(note, -1);
                assert!((velocity - 0.79).abs() < 1e-9);
            }
            other => panic!("expected Trigger, got {:?}", other),
        }
    }

    #[test]
    fn unresolvable_name_target_is_error() {
        let r = NameResolver::empty();
        assert!(json_to_action(&r, "clear", &json!({ "target": "ghost" })).is_err());
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("{}", QUICK_USAGE);
        std::process::exit(2);
    }

    if args[1] == "--interactive" || args[1] == "-i" {
        let cfg = parse_interactive_args(&args[2..]).unwrap_or_else(|e| {
            eprintln!("[test-driver] {}", e);
            std::process::exit(2);
        });
        match run_interactive(&cfg) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("[test-driver] error: {}", e);
                std::process::exit(2);
            }
        }
    }

    let (scenario, baseline_mode) = if args[1].starts_with('-') {
        let s = parse_quick_args(&args[1..]).unwrap_or_else(|e| {
            eprintln!("[test-driver] {}", e);
            std::process::exit(2);
        });
        (s, BaselineMode::Off)
    } else {
        let yaml_path = &args[1];
        let yaml = std::fs::read_to_string(yaml_path)
            .unwrap_or_else(|e| {
                eprintln!("[test-driver] cannot read {}: {}", yaml_path, e);
                std::process::exit(2);
            });
        let s = scenario::parse_scenario(&yaml)
            .unwrap_or_else(|e| {
                eprintln!("[test-driver] {}", e);
                std::process::exit(2);
            });
        let mode = parse_baseline_flag(&args[2..], yaml_path).unwrap_or_else(|e| {
            eprintln!("[test-driver] {}", e);
            std::process::exit(2);
        });
        (s, mode)
    };

    let result = match baseline_mode {
        BaselineMode::Off => run_batch(scenario),
        mode => run_baseline(scenario, mode),
    };

    match result {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            if e.contains("assertions failed") {
                std::process::exit(1);
            }
            eprintln!("[test-driver] error: {}", e);
            std::process::exit(2);
        }
    }
}
