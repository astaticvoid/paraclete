//! App-level operation execution.
//!
//! P11 C1: the drain site and stub `execute` function.  Real execution
//! (kit capture/apply, temp save/reload, mute toggles) lands in C2–C5.
//! Until then, every `AppOp` variant logs and returns.

use log::debug;
use paraclete_node_api::app_op::AppOp;
use paraclete_runtime::NodeConfigurator;
use std::cell::RefCell;
use std::rc::Rc;

/// Execute one app-level operation.  Stub in C1 — logs and returns;
/// C2–C5 fill in each variant.
pub fn execute_app_op(
    op: AppOp,
    _conf: &mut NodeConfigurator,
    _bus: &Rc<RefCell<paraclete_node_api::StateBusHandle>>,
) {
    debug!("[app_ops] stub execute: {op:?}");
}
