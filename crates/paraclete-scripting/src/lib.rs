// SPDX-License-Identifier: GPL-3.0-or-later
//! Paraclete L4 Scripting — live runtime scripting via Rhai.
//!
//! Scripts run exclusively off the audio thread. They can read and write
//! authorised state bus addresses, call `Scriptable` entry points on nodes,
//! and define / redefine hardware mappings without restart.
//!
//! At P0 the engine is sandboxed with no bindings to platform internals.
//! Bindings ship in Phase 1 alongside the first real hardware events.

use rhai::{Engine, EvalAltResult, Scope, AST};

/// A sandboxed Rhai scripting engine.
///
/// Created once; lives on the main thread. Scripts are evaluated on demand.
/// Bindings to platform APIs are added as the platform matures.
pub struct ScriptingEngine {
    engine: Engine,
}

impl ScriptingEngine {
    /// Create a sandboxed scripting engine.
    ///
    /// The sandbox enforces:
    /// - Bounded operation count to prevent infinite loops
    /// - Bounded call depth to prevent stack overflow
    /// - No `while` / `loop` / `for` (optional — enabled for complex mappings)
    pub fn new() -> Self {
        let mut engine = Engine::new();

        // Hard operation budget per script call.
        // 100k ops ≈ a few milliseconds of script execution.
        engine.set_max_operations(100_000);

        // Prevent deeply recursive scripts from blowing the stack.
        engine.set_max_call_levels(32);

        // Limit expression nesting depth to catch malformed scripts early.
        engine.set_max_expr_depths(64, 32);

        log::info!("scripting engine initialised (sandboxed, no bindings at P0)");

        Self { engine }
    }

    /// Compile a script source string to an AST for repeated evaluation.
    pub fn compile(&self, source: &str) -> Result<AST, Box<EvalAltResult>> {
        self.engine.compile(source).map_err(|e| e.into())
    }

    /// Evaluate a pre-compiled AST with a fresh variable scope.
    pub fn run(&self, ast: &AST) -> Result<(), Box<EvalAltResult>> {
        let mut scope = Scope::new();
        self.engine.run_ast_with_scope(&mut scope, ast)
    }

    /// Compile and immediately evaluate a source string.
    pub fn eval_str(&self, source: &str) -> Result<(), Box<EvalAltResult>> {
        let ast = self.compile(source)?;
        self.run(&ast)
    }
}

impl Default for ScriptingEngine {
    fn default() -> Self {
        Self::new()
    }
}
