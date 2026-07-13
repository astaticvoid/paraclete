// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L4 Scripting — live runtime scripting via Rhai.
//!
//! Scripts run exclusively on the main thread. Each profile loads into an
//! isolated `ScriptContext`. Hardware events are dispatched to registered
//! handlers. State bus subscriptions fire callbacks on value changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use rhai::{Dynamic, Engine, EvalAltResult, FnPtr, Scope, AST};

use paraclete_node_api::{
    SurfaceEvent, SurfaceEventMsg, SurfaceOutput, LedUpdate, NodeCommand,
    RgbColor, StateBusHandle, StateBusValue,
};

// ── EventSignature and MacroBinding ──────────────────────────────────────────

#[derive(Clone, Debug)]
pub struct EventSignature {
    pub device_id:  u32,
    pub event_type: String,
    pub control_id: u32,
}

#[derive(Clone, Debug)]
struct MacroBinding {
    signature: EventSignature,
    macro_name: String,
}

// ── OwnedSubscription ─────────────────────────────────────────────────────────

struct OwnedSubscription {
    path: String,
    last_value: Option<StateBusValue>,
    callback: FnPtr,
    last_written_at: Instant,
}

// ── ScriptContext ─────────────────────────────────────────────────────────────

/// One named profile script's runtime state. Isolated from other contexts.
pub struct ScriptContext {
    pub name: String,
    ast: AST,
    /// Reserved for per-context Rhai variable persistence (script-local scope
    /// isolation between contexts). Constructed but not yet read — the eval path
    /// does not thread it through yet. Kept as the wiring point for that feature.
    #[allow(dead_code)]
    scope: Scope<'static>,
}

// ── Shared mutable state (accessible from Rhai builtins) ─────────────────────

struct ScriptState {
    current_context: String,
    /// Per-context: hw_handlers, macros, bindings, subscriptions
    context_data: HashMap<String, ContextData>,
    pending_commands: Vec<NodeCommand>,
    pending_output: HashMap<u32, SurfaceOutput>,
    state_bus: Option<Rc<RefCell<StateBusHandle>>>,
}

struct ContextData {
    hw_handlers: Vec<(u32, FnPtr)>,
    macros:      HashMap<String, FnPtr>,
    bindings:    Vec<MacroBinding>,
    subscriptions: Vec<OwnedSubscription>,
}

impl ContextData {
    fn new() -> Self {
        Self {
            hw_handlers: Vec::new(),
            macros:      HashMap::new(),
            bindings:    Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn clear(&mut self) {
        self.hw_handlers.clear();
        self.macros.clear();
        self.bindings.clear();
        self.subscriptions.clear();
    }
}

impl ScriptState {
    fn new() -> Self {
        Self {
            current_context: String::new(),
            context_data: HashMap::new(),
            pending_commands: Vec::new(),
            pending_output: HashMap::new(),
            state_bus: None,
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn event_type_str(ev: &SurfaceEvent) -> &'static str {
    match ev {
        SurfaceEvent::PadPressed { .. }     => "PadPressed",
        SurfaceEvent::PadReleased { .. }    => "PadReleased",
        SurfaceEvent::PadPressure { .. }    => "PadPressure",
        SurfaceEvent::ButtonPressed { .. }  => "ButtonPressed",
        SurfaceEvent::ButtonReleased { .. } => "ButtonReleased",
        SurfaceEvent::EncoderChanged { .. } => "EncoderChanged",
        SurfaceEvent::EncoderPush { .. }    => "EncoderPush",
        SurfaceEvent::FaderMoved { .. }     => "FaderMoved",
    }
}

fn event_control_id(ev: &SurfaceEvent) -> u32 {
    match ev {
        SurfaceEvent::PadPressed { id, .. }     => *id,
        SurfaceEvent::PadReleased { id }        => *id,
        SurfaceEvent::PadPressure { id, .. }    => *id,
        SurfaceEvent::ButtonPressed { id }      => *id,
        SurfaceEvent::ButtonReleased { id }     => *id,
        SurfaceEvent::EncoderChanged { id, .. } => *id,
        SurfaceEvent::EncoderPush { id, .. }    => *id,
        SurfaceEvent::FaderMoved { id, .. }     => *id,
    }
}

fn event_to_dynamic(msg: &SurfaceEventMsg) -> Dynamic {
    let mut map = rhai::Map::new();
    let ev = &msg.event;
    map.insert("device_id".into(),  Dynamic::from(msg.device_id as i64));
    map.insert("event_type".into(), Dynamic::from(event_type_str(ev).to_string()));
    map.insert("id".into(), Dynamic::from(event_control_id(ev) as i64));
    map.insert("row".into(), Dynamic::from(event_control_id(ev) as i64 / 8));
    map.insert("col".into(), Dynamic::from(event_control_id(ev) as i64 % 8));
    match ev {
        SurfaceEvent::PadPressed { velocity, pressure, .. } => {
            map.insert("velocity".into(), Dynamic::from(*velocity as i64));
            map.insert("pressure".into(), Dynamic::from(*pressure as i64));
        }
        SurfaceEvent::PadPressure { pressure, .. } => {
            map.insert("pressure".into(), Dynamic::from(*pressure as i64));
        }
        SurfaceEvent::EncoderChanged { value, delta, .. } => {
            map.insert("value".into(), Dynamic::from(*value as i64));
            map.insert("delta".into(), Dynamic::from(*delta as i64));
        }
        SurfaceEvent::EncoderPush { pressed, .. } => {
            map.insert("pressed".into(), Dynamic::from(*pressed));
        }
        SurfaceEvent::FaderMoved { value, .. } => {
            map.insert("value".into(), Dynamic::from(*value as i64));
        }
        _ => {}
    }
    Dynamic::from(map)
}

fn state_to_dynamic(v: &StateBusValue) -> Dynamic {
    match v {
        StateBusValue::Float(f) => Dynamic::from(*f),
        StateBusValue::Int(i)   => Dynamic::from(*i),
        StateBusValue::Bool(b)  => Dynamic::from(*b),
        StateBusValue::Text(s)  => Dynamic::from(s.clone()),
    }
}

// ── Builtin registration ──────────────────────────────────────────────────────

fn register_builtins(engine: &mut Engine, state: Rc<RefCell<ScriptState>>) {
    // ── state_read(path) → Dynamic ────────────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("state_read", move |path: &str| -> Dynamic {
            let st = s.borrow();
            if let Some(bus) = &st.state_bus {
                let bus = bus.borrow();
                return bus.read(path).map(state_to_dynamic).unwrap_or(Dynamic::UNIT);
            }
            Dynamic::UNIT
        });
    }

    // ── state_write(path, value) ──────────────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("state_write", move |path: &str, value: Dynamic| {
            let st = s.borrow();
            if let Some(bus) = &st.state_bus {
                let mut bus = bus.borrow_mut();
                // Check bool before int — Rhai bools can coerce to int.
                // Use is::<ImmutableString>() to detect Rhai strings correctly;
                // try_cast::<String>() fails on ImmutableString.
                let sv = if let Ok(b) = value.as_bool() {
                    StateBusValue::Bool(b)
                } else if let Ok(i) = value.as_int() {
                    StateBusValue::Int(i)
                } else if let Ok(f) = value.as_float() {
                    StateBusValue::Float(f)
                } else if value.is::<rhai::ImmutableString>() {
                    StateBusValue::Text(value.cast::<rhai::ImmutableString>().to_string())
                } else {
                    return;
                };
                let _ = bus.write_sandboxed(path, sv);
            }
        });
    }

    // ── publish_context(encoder_key, node_id, param_name) ────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("publish_context", move |encoder_key: &str, node_id: i64, param_name: &str| {
            use paraclete_node_api::ParamDescriptor;
            let param_hash = ParamDescriptor::id_for_name(param_name) as f64;
            let node_path  = format!("/context/{}/node",  encoder_key);
            let param_path = format!("/context/{}/param", encoder_key);
            let st = s.borrow();
            if let Some(bus) = &st.state_bus {
                let mut bus = bus.borrow_mut();
                bus.write(&node_path,  StateBusValue::Float(node_id as f64));
                bus.write(&param_path, StateBusValue::Float(param_hash));
            }
        });
    }

    // ── send_cmd(node_id, type_id, arg0, arg1) ────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("send_cmd", move |target_id: i64, type_id: i64, arg0: i64, arg1: f64| {
            s.borrow_mut().pending_commands.push(NodeCommand {
                target_id: target_id as u32,
                type_id:   type_id as u32,
                arg0,
                arg1,
            });
        });
    }

    // ── set_led(device_id, control_id, r, g, b) ───────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("set_led", move |device_id: i64, control_id: i64, r: i64, g: i64, b: i64| {
            let mut st = s.borrow_mut();
            let entry = st.pending_output
                .entry(device_id as u32)
                .or_insert_with(SurfaceOutput::empty);
            entry.led_updates.push(LedUpdate {
                control_id: control_id as u32,
                color: RgbColor { r: r as u8, g: g as u8, b: b as u8 },
            });
        });
    }

    // ── on_surface_event(device_id, fn) ────────────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("on_surface_event", move |device_id: i64, handler: FnPtr| {
            let mut st = s.borrow_mut();
            let ctx_name = st.current_context.clone();
            let ctx = st.context_data.entry(ctx_name).or_insert_with(ContextData::new);
            ctx.hw_handlers.push((device_id as u32, handler));
        });
    }

    // ── on_surface_event([device_ids…], fn) ───────────────────────────────────
    // Array overload: one inline handler serves any number of surfaces
    // (Rhai's FnPtr-from-closure cannot be stored and re-passed, so profiles
    // cannot register the same closure twice themselves).
    {
        let s = Rc::clone(&state);
        engine.register_fn("on_surface_event", move |device_ids: rhai::Array, handler: FnPtr| {
            let mut st = s.borrow_mut();
            let ctx_name = st.current_context.clone();
            let ctx = st.context_data.entry(ctx_name).or_insert_with(ContextData::new);
            // Dedupe within the call: dispatch fires every matching entry, so
            // a repeated id (e.g. two absent devices both injected as 0)
            // would double-fire the handler — toggle handlers would look dead.
            let mut seen: Vec<u32> = Vec::with_capacity(device_ids.len());
            for id in device_ids {
                match id.as_int() {
                    Ok(i) if i >= 0 => {
                        let i = i as u32;
                        if !seen.contains(&i) {
                            seen.push(i);
                            ctx.hw_handlers.push((i, handler.clone()));
                        }
                    }
                    Ok(i) => eprintln!("[rhai] on_surface_event: negative device id {i} ignored"),
                    Err(t) => eprintln!("[rhai] on_surface_event: non-integer device id ({t})"),
                }
            }
        });
    }

    // ── subscribe(path, fn) ───────────────────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("subscribe", move |path: &str, callback: FnPtr| {
            let mut st = s.borrow_mut();
            let initial = st.state_bus.as_ref()
                .and_then(|b| b.borrow().read(path).cloned());
            let ctx_name = st.current_context.clone();
            let ctx = st.context_data.entry(ctx_name).or_insert_with(ContextData::new);
            ctx.subscriptions.push(OwnedSubscription {
                path: path.to_string(),
                last_value: initial,
                callback,
                last_written_at: Instant::now(),
            });
        });
    }

    // ── state_alive(path, timeout_ms) → bool ─────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("state_alive", move |path: &str, timeout_ms: i64| -> bool {
            let st = s.borrow();
            if let Some(ctx) = st.context_data.get(&st.current_context) {
                if let Some(sub) = ctx.subscriptions.iter().find(|sub| sub.path == path) {
                    return sub.last_written_at.elapsed().as_millis() < timeout_ms as u128;
                }
            }
            false
        });
    }

    // ── def_macro(name, fn) ───────────────────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("def_macro", move |name: &str, f: FnPtr| {
            let mut st = s.borrow_mut();
            let ctx_name = st.current_context.clone();
            let ctx = st.context_data.entry(ctx_name).or_insert_with(ContextData::new);
            ctx.macros.insert(name.to_string(), f);
        });
    }

    // ── bind_macro(device_id, event_type, control_id, macro_name) ────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("bind_macro", move |device_id: i64, event_type: &str, control_id: i64, macro_name: &str| {
            let mut st = s.borrow_mut();
            let ctx_name = st.current_context.clone();
            let ctx = st.context_data.entry(ctx_name).or_insert_with(ContextData::new);
            ctx.bindings.push(MacroBinding {
                signature: EventSignature {
                    device_id:  device_id as u32,
                    event_type: event_type.to_string(),
                    control_id: control_id as u32,
                },
                macro_name: macro_name.to_string(),
            });
        });
    }

    // ── fire_macro(name) ──────────────────────────────────────────────────────
    // Note: firing a macro requires calling a FnPtr which needs the AST.
    // The actual fire_macro call is handled in dispatch_surface_event().
    // Here we register a no-op placeholder so scripts don't error on the call.
    {
        engine.register_fn("fire_macro", |_name: &str| {
            // Actual dispatch done in ScriptingEngine::dispatch_surface_event
        });
    }

    // ── debug_print(msg) ──────────────────────────────────────────────────────
    engine.register_fn("debug_print", |msg: &str| {
        eprintln!("[rhai] {msg}");
    });

    // ── get_step_state(node_id) → String ─────────────────────────────────────
    {
        let s = Rc::clone(&state);
        engine.register_fn("get_step_state", move |node_id: i64| -> String {
            let st = s.borrow();
            if let Some(bus) = &st.state_bus {
                let bus = bus.borrow();
                let path = format!("/node/{}/state/steps", node_id);
                if let Some(StateBusValue::Text(bits)) = bus.read(&path) {
                    return bits.clone();
                }
            }
            String::new()
        });
    }

    // ── reload_profile(name) — handled externally; stub here ─────────────────
    engine.register_fn("reload_profile", |_name: &str| {});

    // ── assert helper ─────────────────────────────────────────────────────────
    engine.register_fn("assert", |condition: bool| {
        if !condition {
            panic!("rhai assertion failed");
        }
    });
}

// ── Legacy StateBusProxy (preserved for compatibility) ───────────────────────

#[derive(Clone)]
pub struct StateBusProxy {
    handle: Rc<RefCell<StateBusHandle>>,
}

impl StateBusProxy {
    fn new(handle: Rc<RefCell<StateBusHandle>>) -> Self { Self { handle } }

    pub fn read(&mut self, path: &str) -> Dynamic {
        let handle = self.handle.borrow();
        match handle.read(path) {
            Some(StateBusValue::Float(f)) => Dynamic::from(*f),
            Some(StateBusValue::Int(i))   => Dynamic::from(*i),
            Some(StateBusValue::Bool(b))  => Dynamic::from(*b),
            Some(StateBusValue::Text(s))  => Dynamic::from(s.clone()),
            None                          => Dynamic::UNIT,
        }
    }

    pub fn write(&mut self, path: &str, value: f64) {
        let _ = self.handle.borrow_mut().write_sandboxed(path, StateBusValue::Float(value));
    }

    pub fn subscribe(&mut self, path: &str) -> StateBusSubscriptionProxy {
        StateBusSubscriptionProxy {
            handle: Rc::clone(&self.handle),
            path: path.to_string(),
            last_value: None,
        }
    }
}

#[derive(Clone)]
pub struct StateBusSubscriptionProxy {
    handle: Rc<RefCell<StateBusHandle>>,
    path: String,
    last_value: Option<StateBusValue>,
}

impl StateBusSubscriptionProxy {
    pub fn changed(&mut self) -> bool {
        let handle  = self.handle.borrow();
        let current = handle.read(&self.path).cloned();
        if current != self.last_value {
            self.last_value = current;
            true
        } else {
            false
        }
    }

    pub fn value(&mut self) -> Dynamic {
        match &self.last_value {
            Some(StateBusValue::Float(f)) => Dynamic::from(*f),
            Some(StateBusValue::Int(i))   => Dynamic::from(*i),
            Some(StateBusValue::Bool(b))  => Dynamic::from(*b),
            Some(StateBusValue::Text(s))  => Dynamic::from(s.clone()),
            None                          => Dynamic::UNIT,
        }
    }
}

fn register_legacy_state_bus(engine: &mut Engine) {
    engine.register_type_with_name::<StateBusProxy>("StateBus");
    engine.register_fn("read",      StateBusProxy::read);
    engine.register_fn("write",     StateBusProxy::write);
    engine.register_fn("subscribe", StateBusProxy::subscribe);

    engine.register_type_with_name::<StateBusSubscriptionProxy>("StateBusSubscription");
    engine.register_fn("changed", StateBusSubscriptionProxy::changed);
    engine.register_fn("value",   StateBusSubscriptionProxy::value);
}

// ── ScriptingEngine ───────────────────────────────────────────────────────────

/// The main scripting runtime. Lives on the main thread only (`!Send`).
pub struct ScriptingEngine {
    engine: Engine,
    script_state: Rc<RefCell<ScriptState>>,
    contexts: HashMap<String, ScriptContext>,
    /// Legacy state bus proxy for backwards-compatible scripts.
    state_bus_proxy: Option<StateBusProxy>,
}

impl ScriptingEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(500_000);
        engine.set_max_call_levels(64);
        engine.set_max_expr_depths(128, 64);

        let script_state = Rc::new(RefCell::new(ScriptState::new()));
        register_builtins(&mut engine, Rc::clone(&script_state));
        register_legacy_state_bus(&mut engine);

        log::info!("scripting engine initialised");

        Self {
            engine,
            script_state,
            contexts: HashMap::new(),
            state_bus_proxy: None,
        }
    }

    /// Bind the shared state bus handle (for both legacy and new builtins).
    pub fn bind_state_bus(&mut self, handle: Rc<RefCell<StateBusHandle>>) {
        self.script_state.borrow_mut().state_bus = Some(Rc::clone(&handle));
        self.state_bus_proxy = Some(StateBusProxy::new(handle));
    }

    /// Evaluate a script file into a named `ScriptContext`.
    /// If a context with this name already exists, it is torn down first.
    /// Calls `on_load()` in the script if defined.
    pub fn eval_file(
        &mut self,
        name: &str,
        path: &str,
        constants: &[(String, Dynamic)],
    ) -> Result<(), Box<EvalAltResult>> {
        // Tear down existing context.
        if let Some(ctx_data) = self.script_state.borrow_mut().context_data.get_mut(name) {
            ctx_data.clear();
        }
        self.contexts.remove(name);

        // Set current context for builtin registration.
        self.script_state.borrow_mut().current_context = name.to_string();
        // Ensure the context data slot exists.
        self.script_state.borrow_mut().context_data
            .entry(name.to_string()).or_insert_with(ContextData::new);

        // Build scope with injected constants.
        let mut scope = Scope::new();
        for (k, v) in constants {
            scope.push_constant(k.as_str(), v.clone());
        }
        if let Some(ref proxy) = self.state_bus_proxy {
            scope.push("state_bus", proxy.clone());
        }

        // Compile and run.
        let source = std::fs::read_to_string(path)
            .map_err(|e| -> Box<EvalAltResult> {
                Box::new(EvalAltResult::ErrorSystem(
                    format!("cannot read {path}: {e}"),
                    Box::new(e),
                ))
            })?;
        let ast = self.engine.compile(&source)?;
        self.engine.run_ast_with_scope(&mut scope, &ast)?;

        // Call on_load() if defined (ignore "function not found" errors).
        if let Err(e) = self.engine.call_fn::<()>(&mut scope, &ast, "on_load", ()) {
            if !matches!(*e, EvalAltResult::ErrorFunctionNotFound(ref n, _) if n.starts_with("on_load")) {
                eprintln!("[rhai] on_load error ({name}): {e}");
            }
        }

        self.contexts.insert(name.to_string(), ScriptContext {
            name: name.to_string(),
            ast,
            scope,
        });

        Ok(())
    }

    /// Dispatch a hardware event to all matching handlers and macro bindings.
    pub fn dispatch_surface_event(&mut self, msg: &SurfaceEventMsg) {
        let event_type_s = event_type_str(&msg.event).to_string();
        let control_id   = event_control_id(&msg.event);
        let event_dyn    = event_to_dynamic(msg);

        let ctx_names: Vec<String> = self.contexts.keys().cloned().collect();
        for ctx_name in &ctx_names {
            // Collect matching handlers (clone to avoid borrow conflict).
            let handlers: Vec<FnPtr> = {
                let st = self.script_state.borrow();
                st.context_data.get(ctx_name)
                    .map(|d| d.hw_handlers.iter()
                        .filter(|(dev_id, _)| *dev_id == msg.device_id)
                        .map(|(_, fp)| fp.clone())
                        .collect())
                    .unwrap_or_default()
            };

            if let Some(ctx) = self.contexts.get_mut(ctx_name) {
                for handler in &handlers {
                    // FnPtr::call(engine, ast, args) — scope is embedded in captured closures.
                    if let Err(e) = handler.call::<()>(
                        &self.engine,
                        &ctx.ast,
                        (event_dyn.clone(),),
                    ) {
                        eprintln!("[rhai] surface-event handler error ({ctx_name}): {e}");
                    }
                }
            }

            // Macro bindings.
            let matching_macro: Option<FnPtr> = {
                let st = self.script_state.borrow();
                st.context_data.get(ctx_name).and_then(|d| {
                    d.bindings.iter()
                        .find(|b|
                            b.signature.device_id  == msg.device_id &&
                            b.signature.event_type == event_type_s  &&
                            b.signature.control_id == control_id)
                        .and_then(|b| d.macros.get(&b.macro_name).cloned())
                })
            };

            if let Some(macro_fn) = matching_macro {
                if let Some(ctx) = self.contexts.get_mut(ctx_name) {
                    let _ = macro_fn.call::<()>(
                        &self.engine,
                        &ctx.ast,
                        (),
                    );
                }
            }
        }
    }

    /// Fire subscription callbacks for any state bus paths that changed.
    /// Call after `process_main_thread()` has drained the state bus.
    ///
    /// Takes the shared handle (not a borrowed `&StateBusHandle`) so the bus
    /// is only borrowed around each path read — callbacks are free to call
    /// `state_write`, which needs `borrow_mut` on the same `RefCell`. A
    /// caller-held borrow across the dispatch panicked at the first
    /// subscription callback that wrote state (found 2026-07-11 when
    /// `launchpad.rhai` began publishing `/script/lp/steps_n` from its steps
    /// subscription).
    pub fn process_subscriptions(&mut self, state_bus: &Rc<RefCell<StateBusHandle>>) {
        for (ctx_name, ctx) in &mut self.contexts {
            let subs_len = self.script_state.borrow()
                .context_data.get(ctx_name.as_str())
                .map(|d| d.subscriptions.len())
                .unwrap_or(0);

            for i in 0..subs_len {
                let (path, last_val) = {
                    let st = self.script_state.borrow();
                    let d = &st.context_data[ctx_name.as_str()];
                    (d.subscriptions[i].path.clone(), d.subscriptions[i].last_value.clone())
                };

                // Borrow scoped to this statement: dropped before the
                // callback below can state_write (borrow_mut).
                let current = state_bus.borrow().read(&path).cloned();
                if current != last_val {
                    // Fire callback.
                    let (fp, new_ts) = {
                        let st = self.script_state.borrow();
                        let d = &st.context_data[ctx_name.as_str()];
                        (d.subscriptions[i].callback.clone(), Instant::now())
                    };

                    let dyn_val = current.as_ref().map(state_to_dynamic).unwrap_or(Dynamic::UNIT);
                    let _ = fp.call::<()>(
                        &self.engine,
                        &ctx.ast,
                        (dyn_val,),
                    );

                    // Update last_value and last_written_at.
                    if let Some(d) = self.script_state.borrow_mut()
                        .context_data.get_mut(ctx_name.as_str()) {
                        d.subscriptions[i].last_value      = current;
                        d.subscriptions[i].last_written_at = new_ts;
                    }
                }
            }
        }
    }

    /// Drain and return accumulated NodeCommands from `send_cmd()` calls.
    pub fn take_pending_commands(&mut self) -> Vec<NodeCommand> {
        let mut cmds = Vec::new();
        std::mem::swap(&mut cmds, &mut self.script_state.borrow_mut().pending_commands);
        cmds
    }

    /// Drain and return accumulated LED output from `set_led()` calls.
    pub fn take_pending_output(&mut self) -> HashMap<u32, SurfaceOutput> {
        let mut out = HashMap::new();
        std::mem::swap(&mut out, &mut self.script_state.borrow_mut().pending_output);
        out
    }

    // ── Legacy API (P3 compatibility) ─────────────────────────────────────────

    pub fn compile(&self, source: &str) -> Result<AST, Box<EvalAltResult>> {
        self.engine.compile(source).map_err(|e| e.into())
    }

    pub fn run(&self, ast: &AST) -> Result<(), Box<EvalAltResult>> {
        let mut scope = Scope::new();
        if let Some(ref proxy) = self.state_bus_proxy {
            scope.push("state_bus", proxy.clone());
        }
        self.engine.run_ast_with_scope(&mut scope, ast)
    }

    pub fn eval_str(&self, source: &str) -> Result<(), Box<EvalAltResult>> {
        let ast = self.compile(source)?;
        self.run(&ast)
    }
}

impl Default for ScriptingEngine {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_engine_with_bus() -> (ScriptingEngine, Rc<RefCell<StateBusHandle>>) {
        let handle = Rc::new(RefCell::new(StateBusHandle::new()));
        let mut engine = ScriptingEngine::new();
        engine.bind_state_bus(Rc::clone(&handle));
        (engine, handle)
    }

    #[test]
    fn scripting_engine_eval_simple_expression() {
        let engine = ScriptingEngine::new();
        assert!(engine.eval_str("let x = 1 + 1;").is_ok());
    }

    #[test]
    fn scripting_engine_state_bus_read_returns_unit_for_unknown_path() {
        let (engine, _handle) = make_engine_with_bus();
        engine.eval_str(r#"
            let v = state_bus.read("/transport/bpm");
            assert(type_of(v) == "()");
        "#).expect("script failed");
    }

    #[test]
    fn scripting_engine_state_bus_write_then_read() {
        let (engine, handle) = make_engine_with_bus();
        handle.borrow_mut().write("/node/1/param/pitch", StateBusValue::Float(2.0));
        engine.eval_str(r#"
            let v = state_bus.read("/node/1/param/pitch");
            assert(v == 2.0);
        "#).expect("script failed");
    }

    #[test]
    fn scripting_engine_state_bus_write_to_node_path_succeeds() {
        let (engine, handle) = make_engine_with_bus();
        assert!(engine.eval_str(r#"
            state_bus.write("/node/1/param/pitch", 3.0);
        "#).is_ok());
        assert_eq!(
            handle.borrow().read("/node/1/param/pitch"),
            Some(&StateBusValue::Float(3.0)),
        );
    }

    #[test]
    fn scripting_engine_state_bus_write_to_transport_is_rejected() {
        let (engine, handle) = make_engine_with_bus();
        assert!(engine.eval_str(r#"
            state_bus.write("/transport/bpm", 140.0);
        "#).is_ok());
        assert!(handle.borrow().read("/transport/bpm").is_none());
    }

    #[test]
    fn scripting_engine_state_bus_subscribe_detects_change() {
        let (engine, handle) = make_engine_with_bus();
        handle.borrow_mut().write("/node/1/state/step", StateBusValue::Int(3));
        engine.eval_str(r#"
            let sub = state_bus.subscribe("/node/1/state/step");
            assert(sub.changed());
        "#).expect("script failed");
    }

    #[test]
    fn state_read_builtin_returns_value() {
        let (engine, handle) = make_engine_with_bus();
        handle.borrow_mut().write("/node/1/state/x", StateBusValue::Float(42.0));
        assert!(engine.eval_str(r#"
            let v = state_read("/node/1/state/x");
        "#).is_ok());
    }

    #[test]
    fn on_surface_event_array_registers_handler_for_multiple_devices() {
        let mut engine = ScriptingEngine::new();
        let path = std::env::temp_dir().join("on_surface_event_array_test.rhai");
        // Registration lives in on_load(), the production idiom — top-level
        // statements run twice (run_ast + call_fn's AST evaluation).
        // 42 repeated + a negative id: both must not double-register.
        std::fs::write(&path, r#"
            fn on_load() {
                on_surface_event([42, 77, 42, -3], |event| {
                    send_cmd(1, 16, event.id, 0.0);
                });
            }
        "#).unwrap();
        engine.eval_file("array_test", path.to_str().unwrap(), &[])
            .expect("script with array registration must load");

        use paraclete_node_api::{SurfaceEvent, SurfaceEventMsg};
        for (dev, pad) in [(42u32, 5i64), (77, 6), (99, 7)] {
            engine.dispatch_surface_event(&SurfaceEventMsg {
                device_id: dev,
                event: SurfaceEvent::PadPressed { id: pad as u32, velocity: 100, pressure: 0 },
            });
        }

        let cmds: Vec<_> = engine.take_pending_commands().into_iter()
            .filter(|c| c.target_id == 1 && c.type_id == 16)
            .collect();
        assert_eq!(cmds.len(), 2, "handler fires for both registered ids, not the third");
        assert_eq!(cmds[0].arg0, 5, "device 42's event dispatched");
        assert_eq!(cmds[1].arg0, 6, "device 77's event dispatched");
    }

    #[test]
    fn send_cmd_accumulates_node_commands() {
        let mut engine = ScriptingEngine::new();
        engine.eval_str("send_cmd(5, 0, 10, 0.5);").expect("send_cmd failed");
        let real: Vec<_> = engine.take_pending_commands().into_iter()
            .filter(|c| c.target_id == 5 && c.type_id == 0)
            .collect();
        assert_eq!(real.len(), 1);
        assert_eq!(real[0].target_id, 5);
        assert_eq!(real[0].arg0, 10);
        assert!((real[0].arg1 - 0.5).abs() < 1e-9);
    }

    // ── state_write string regression ────────────────────────────────────────
    // Root cause of the "mode never switches" bug: try_cast::<String>() fails
    // on Rhai's ImmutableString type, so string writes were silently dropped.

    #[test]
    fn state_write_string_roundtrips() {
        let (engine, handle) = make_engine_with_bus();
        engine.eval_str(r#"
            state_write("/node/1/param/mode", "sequence");
        "#).expect("state_write string failed");
        assert_eq!(
            handle.borrow().read("/node/1/param/mode"),
            Some(&StateBusValue::Text("sequence".into())),
            "state_write with a string value must persist to the state bus"
        );
    }

    #[test]
    fn state_write_overwrites_string() {
        let (engine, handle) = make_engine_with_bus();
        engine.eval_str(r#"
            state_write("/node/1/param/mode", "trigger");
            state_write("/node/1/param/mode", "sequence");
        "#).expect("script failed");
        assert_eq!(
            handle.borrow().read("/node/1/param/mode"),
            Some(&StateBusValue::Text("sequence".into()))
        );
    }

    #[test]
    fn state_read_after_write_string_returns_correct_value() {
        let (engine, handle) = make_engine_with_bus();
        handle.borrow_mut().write("/node/1/param/selected", StateBusValue::Int(3));
        engine.eval_str(r#"
            state_write("/node/1/param/mode", "sequence");
            let m = state_read("/node/1/param/mode");
            assert(m == "sequence");
            let t = state_read("/node/1/param/selected");
            assert(t == 3);
        "#).expect("state read/write roundtrip failed");
    }

    #[test]
    fn set_led_accumulates_output() {
        let mut engine = ScriptingEngine::new();
        engine.eval_str("set_led(1, 5, 64, 128, 255);").expect("set_led failed");
        let out = engine.take_pending_output();
        assert!(out.contains_key(&1));
        assert_eq!(out[&1].led_updates[0].control_id, 5);
        assert_eq!(out[&1].led_updates[0].color.r, 64);
    }

    #[test]
    fn publish_context_writes_node_and_param_to_state_bus() {
        use paraclete_node_api::ParamDescriptor;
        let (engine, handle) = make_engine_with_bus();
        engine.eval_str(r#"publish_context("encoder_0", 42, "decay");"#)
            .expect("publish_context failed");
        assert_eq!(
            handle.borrow().read("/context/encoder_0/node"),
            Some(&StateBusValue::Float(42.0)),
        );
        assert_eq!(
            handle.borrow().read("/context/encoder_0/param"),
            Some(&StateBusValue::Float(ParamDescriptor::id_for_name("decay") as f64)),
        );
    }

    #[test]
    fn publish_context_overwrites_previous_mapping() {
        use paraclete_node_api::ParamDescriptor;
        let (engine, handle) = make_engine_with_bus();
        engine.eval_str(r#"
            publish_context("encoder_0", 42, "decay");
            publish_context("encoder_0", 99, "cutoff");
        "#).expect("publish_context failed");
        assert_eq!(
            handle.borrow().read("/context/encoder_0/node"),
            Some(&StateBusValue::Float(99.0)),
        );
        assert_eq!(
            handle.borrow().read("/context/encoder_0/param"),
            Some(&StateBusValue::Float(ParamDescriptor::id_for_name("cutoff") as f64)),
        );
    }

    #[test]
    fn publish_context_different_keys_do_not_collide() {
        let (engine, handle) = make_engine_with_bus();
        engine.eval_str(r#"
            publish_context("encoder_0", 10, "decay");
            publish_context("encoder_1", 20, "cutoff");
        "#).expect("publish_context failed");
        assert_eq!(
            handle.borrow().read("/context/encoder_0/node"),
            Some(&StateBusValue::Float(10.0)),
        );
        assert_eq!(
            handle.borrow().read("/context/encoder_1/node"),
            Some(&StateBusValue::Float(20.0)),
        );
    }
}
