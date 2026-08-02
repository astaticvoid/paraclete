//! App-level operation execution.
//!
//! P11 C1: the drain site and stub `execute` function.  Real execution
//! (kit capture/apply, temp save/reload, mute toggles) lands in C2–C5.
//! C2: every `AppOp` now delegates to `PerformState`.
//! C3–C5 fill in the remaining stub variants (TempSave/TempReload,
//! KitCommit/KitReload).

use log::debug;
use paraclete_node_api::app_op::AppOp;
use paraclete_runtime::NodeConfigurator;
use std::cell::RefCell;
use std::rc::Rc;

use crate::perform_state::PerformState;

/// Execute one app-level operation, delegating to `PerformState`.
pub fn execute_app_op(
    op: AppOp,
    perform: &mut PerformState,
    conf: &mut NodeConfigurator,
    bus: &Rc<RefCell<paraclete_node_api::StateBusHandle>>,
) {
    debug!("[app_ops] execute: {op:?}");
    perform.execute(op, conf, bus);
}
