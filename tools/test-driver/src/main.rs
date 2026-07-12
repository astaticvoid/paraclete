mod analysis;
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
    #[allow(dead_code)]
    sample_rate: f32,
    #[allow(dead_code)]
    block_size: usize,
}

fn resolve_target(resolver: &NameResolver, target: &str) -> Result<u32, String> {
    resolver.resolve_required(target)
}

fn dispatch_action(ctx: &mut TestContext, action: &ResolvedActionKind) -> Result<(), String> {
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
    ctx.conf.send_command(cmd).map_err(|_| "command ring buffer full".into())
}

fn param_id_for_name(name: &str) -> u32 {
    ParamDescriptor::id_for_name(name)
}

fn run_batch(scenario: TestScenario) -> Result<(), String> {
    let instrument_path = PathBuf::from(&scenario.instrument);
    let def = load_instrument_definition(&instrument_path)
        .map_err(|e| format!("failed to load instrument: {}", e))?;
    let resolver = NameResolver::from_instrument(&def);

    let sample_rate = scenario.sample_rate;
    let block_size = scenario.block_size;

    let mut conf = NodeConfigurator::new(sample_rate, block_size);
    let libraries = HashMap::new();
    let _ids = build_from_instrument(&def, &mut conf, &libraries)
        .map_err(|e| format!("failed to build graph: {}", e))?;

    let bus_handle = conf.state_bus_handle();
    let executor = conf.build_executor();
    let capture = Arc::new(CaptureRing::new(CAPTURE_RING_CAPACITY));
    let running = Arc::new(AtomicBool::new(true));

    let executor = Arc::new(Mutex::new(executor));
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

    let mut ctx = TestContext {
        conf, executor, bus_handle, capture, running,
        resolver, sample_rate, block_size,
    };

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
            dispatch_action(&mut ctx, &action.1)?;
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

    if scenario.play {
        let output = scenario.output.clone();
        let status = std::process::Command::new("afplay")
            .arg(&output)
            .status();
        if let Err(e) = status {
            eprintln!("[test-driver] afplay failed: {} (output at {})", e, output);
        }
    }

    Ok(())
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

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("usage: test-driver <test.yaml>");
        std::process::exit(2);
    }

    let yaml_path = &args[1];
    let yaml = std::fs::read_to_string(yaml_path)
        .unwrap_or_else(|e| {
            eprintln!("[test-driver] cannot read {}: {}", yaml_path, e);
            std::process::exit(2);
        });

    let scenario = scenario::parse_scenario(&yaml)
        .unwrap_or_else(|e| {
            eprintln!("[test-driver] {}", e);
            std::process::exit(2);
        });

    match run_batch(scenario) {
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
