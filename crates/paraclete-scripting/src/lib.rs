// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L4 Scripting — live runtime scripting via Rhai.
//!
//! Scripts run exclusively on the main thread. They read and write
//! authorised state bus addresses, call `Scriptable` entry points on nodes,
//! and define / redefine hardware mappings without restart.

use std::cell::RefCell;
use std::rc::Rc;

use rhai::{Engine, EvalAltResult, Scope, AST};

use paraclete_node_api::{StateBusHandle, StateBusValue};

// ── StateBusProxy ─────────────────────────────────────────────────────────────

/// Rhai-visible state bus access object.
///
/// Registered as `StateBus` in the Rhai engine. Scripts receive an instance
/// bound to `state_bus` in their execution scope.
#[derive(Clone)]
pub struct StateBusProxy {
    handle: Rc<RefCell<StateBusHandle>>,
}

impl StateBusProxy {
    fn new(handle: Rc<RefCell<StateBusHandle>>) -> Self {
        Self { handle }
    }

    pub fn read(&mut self, path: &str) -> rhai::Dynamic {
        let handle = self.handle.borrow();
        match handle.read(path) {
            Some(StateBusValue::Float(f)) => rhai::Dynamic::from(*f),
            Some(StateBusValue::Int(i))   => rhai::Dynamic::from(*i),
            Some(StateBusValue::Bool(b))  => rhai::Dynamic::from(*b),
            Some(StateBusValue::Text(s))  => rhai::Dynamic::from(s.clone()),
            None                          => rhai::Dynamic::UNIT,
        }
    }

    pub fn write(&mut self, path: &str, value: f64) {
        let _ = self.handle.borrow_mut()
            .write_sandboxed(path, StateBusValue::Float(value));
    }

    pub fn subscribe(&mut self, path: &str) -> StateBusSubscriptionProxy {
        StateBusSubscriptionProxy {
            handle: Rc::clone(&self.handle),
            path: path.to_string(),
            last_value: None,
        }
    }
}

// ── StateBusSubscriptionProxy ─────────────────────────────────────────────────

/// Rhai-visible subscription handle for a single state bus path.
#[derive(Clone)]
pub struct StateBusSubscriptionProxy {
    handle: Rc<RefCell<StateBusHandle>>,
    path: String,
    last_value: Option<StateBusValue>,
}

impl StateBusSubscriptionProxy {
    pub fn changed(&mut self) -> bool {
        let handle = self.handle.borrow();
        let current = handle.read(&self.path).cloned();
        if current != self.last_value {
            self.last_value = current;
            true
        } else {
            false
        }
    }

    pub fn value(&mut self) -> rhai::Dynamic {
        match &self.last_value {
            Some(StateBusValue::Float(f)) => rhai::Dynamic::from(*f),
            Some(StateBusValue::Int(i))   => rhai::Dynamic::from(*i),
            Some(StateBusValue::Bool(b))  => rhai::Dynamic::from(*b),
            Some(StateBusValue::Text(s))  => rhai::Dynamic::from(s.clone()),
            None                          => rhai::Dynamic::UNIT,
        }
    }
}

// ── State bus API registration ────────────────────────────────────────────────

fn register_state_bus_api(engine: &mut Engine) {
    engine.register_type_with_name::<StateBusProxy>("StateBus");
    engine.register_fn("read", StateBusProxy::read);
    engine.register_fn("write", StateBusProxy::write);
    engine.register_fn("subscribe", StateBusProxy::subscribe);

    engine.register_type_with_name::<StateBusSubscriptionProxy>("StateBusSubscription");
    engine.register_fn("changed", StateBusSubscriptionProxy::changed);
    engine.register_fn("value", StateBusSubscriptionProxy::value);
}

fn register_test_helpers(engine: &mut Engine) {
    engine.register_fn("assert", |condition: bool| {
        if !condition {
            panic!("rhai assertion failed");
        }
    });
}

// ── ScriptingEngine ───────────────────────────────────────────────────────────

/// A sandboxed Rhai scripting engine.
///
/// Created once; lives on the main thread. Scripts are evaluated on demand.
/// Bind the state bus with `bind_state_bus()` to enable `state_bus` access
/// from scripts.
///
/// Note: this type is `!Send` (Rhai without the `sync` feature + `Rc<RefCell<>>`
/// inside `StateBusProxy`). Scripts must only run on the main thread.
pub struct ScriptingEngine {
    engine: Engine,
    state_bus_proxy: Option<StateBusProxy>,
}

impl ScriptingEngine {
    pub fn new() -> Self {
        let mut engine = Engine::new();

        engine.set_max_operations(100_000);
        engine.set_max_call_levels(32);
        engine.set_max_expr_depths(64, 32);

        register_state_bus_api(&mut engine);
        register_test_helpers(&mut engine);

        log::info!("scripting engine initialised");

        Self { engine, state_bus_proxy: None }
    }

    /// Bind the state bus handle so scripts can use `state_bus.read()`,
    /// `state_bus.write()`, and `state_bus.subscribe()`.
    pub fn bind_state_bus(&mut self, handle: Rc<RefCell<StateBusHandle>>) {
        self.state_bus_proxy = Some(StateBusProxy::new(handle));
    }

    /// Compile a script source string to an AST for repeated evaluation.
    pub fn compile(&self, source: &str) -> Result<AST, Box<EvalAltResult>> {
        self.engine.compile(source).map_err(|e| e.into())
    }

    /// Evaluate a pre-compiled AST with a fresh variable scope.
    pub fn run(&self, ast: &AST) -> Result<(), Box<EvalAltResult>> {
        let mut scope = Scope::new();
        if let Some(ref proxy) = self.state_bus_proxy {
            scope.push("state_bus", proxy.clone());
        }
        self.engine.run_ast_with_scope(&mut scope, ast)
    }

    /// Compile and immediately evaluate a source string.
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
        // write_sandboxed silently drops invalid paths — no panic or error in script.
        assert!(engine.eval_str(r#"
            state_bus.write("/transport/bpm", 140.0);
        "#).is_ok());
        // Value must NOT have been written.
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
}
