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

/// Drain one surface's pending app-ops into `PerformState` (P11 C1).
///
/// This is the exact loop body the app main loop runs per surface each tick;
/// factored out so the drain is unit-testable without a running app session.
pub fn drain_app_ops<I>(
    ops: I,
    perform: &mut PerformState,
    conf: &mut NodeConfigurator,
    bus: &Rc<RefCell<paraclete_node_api::StateBusHandle>>,
) where
    I: IntoIterator<Item = AppOp>,
{
    for op in ops {
        execute_app_op(op, perform, conf, bus);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::app_op::KitId;
    use paraclete_runtime::NodeConfigurator;

    /// P11 C1: ops drained from a surface handle land in PerformState in
    /// order — the state changes `execute` makes are observable afterwards.
    #[test]
    fn drain_app_ops_executes_each_op_into_perform_state() {
        let mut conf = NodeConfigurator::new(44100.0, 512);
        let bus = conf.state_bus_handle();
        let mut perform = PerformState::new();

        let ops = vec![
            AppOp::SetPerformMode(true),
            AppOp::BindKit {
                slot: 3,
                kit: Some(KitId(7)),
            },
        ];
        drain_app_ops(ops, &mut perform, &mut conf, &bus);

        assert!(perform.perform_mode, "SetPerformMode op must take effect");
        assert_eq!(
            perform.kit_binding[3],
            Some(KitId(7)),
            "BindKit op must take effect"
        );
    }
}
