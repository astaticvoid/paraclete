// SPDX-License-Identifier: LGPL-3.0-or-later

use crate::capability::CapabilityDocument;
use crate::command::{NodeCommand, CMD_BUMP_PARAM, CMD_SET_PARAM};
use crate::state_bus::StateBusValue;

struct ParameterSlot {
    param_id: u32,
    name:     String,
    current:  f64,
    min:      f64,
    max:      f64,
    default:  f64,
}

/// Pre-allocated parameter storage built from a node's capability document.
///
/// Handles `CMD_SET_PARAM` and `CMD_BUMP_PARAM` with zero audio-thread allocation.
/// Build at `activate()` time; call `handle_commands(input.commands)` before
/// any DSP logic in `process()`.
///
/// Linear scan over slots is correct and efficient for typical parameter counts
/// (< 32). A `HashMap` is not used — it would require allocation at construction
/// on the audio thread.
pub struct ParameterBank {
    slots: Vec<ParameterSlot>,
}

impl ParameterBank {
    /// Build from a capability document. Call at `activate()` time.
    /// Sets `current = default` for all declared parameters.
    pub fn from_capability_document(doc: &CapabilityDocument) -> Self {
        let slots = doc.params.iter().map(|p| ParameterSlot {
            param_id: p.id,
            name:     p.name.as_str().to_string(),
            current:  p.default,
            min:      p.min,
            max:      p.max,
            default:  p.default,
        }).collect();
        Self { slots }
    }

    /// Build an empty bank (no parameters declared).
    pub fn empty() -> Self {
        Self { slots: Vec::new() }
    }

    /// Apply `CMD_SET_PARAM` and `CMD_BUMP_PARAM` from `input.commands`.
    /// All other `type_id` values are silently ignored.
    /// Allocation-free. Call before any DSP logic in `process()`.
    pub fn handle_commands(&mut self, commands: &[NodeCommand]) {
        for cmd in commands {
            if cmd.type_id == CMD_SET_PARAM {
                let param_id = cmd.arg0 as u32;
                if let Some(s) = self.slots.iter_mut().find(|s| s.param_id == param_id) {
                    s.current = cmd.arg1.clamp(s.min, s.max);
                }
            } else if cmd.type_id == CMD_BUMP_PARAM {
                let param_id = cmd.arg0 as u32;
                if let Some(s) = self.slots.iter_mut().find(|s| s.param_id == param_id) {
                    s.current = (s.current + cmd.arg1).clamp(s.min, s.max);
                }
            }
        }
    }

    /// Current value of a parameter. Returns `0.0` if the param_id is not found.
    pub fn get(&self, param_id: u32) -> f64 {
        self.slots.iter()
            .find(|s| s.param_id == param_id)
            .map(|s| s.current)
            .unwrap_or(0.0)
    }

    /// Set a parameter value directly (clamped to declared range).
    /// Use in `activate()`, `deserialize()`, or direct node-internal control.
    pub fn set(&mut self, param_id: u32, value: f64) {
        if let Some(s) = self.slots.iter_mut().find(|s| s.param_id == param_id) {
            s.current = value.clamp(s.min, s.max);
        }
    }

    /// Reset all parameters to their declared defaults.
    pub fn reset(&mut self) {
        for s in &mut self.slots {
            s.current = s.default;
        }
    }

    /// Iterate declared parameters as (name, current_value) pairs.
    /// Only yielded for declared slots.
    pub fn iter_values(&self) -> impl Iterator<Item = (&str, f64)> + '_ {
        self.slots.iter().map(|s| (s.name.as_str(), s.current))
    }
}

/// Push one `/node/{node_id}/{param_name}` = Float(value) entry per declared slot.
/// Appends to `buf`; does not clear it.
pub fn publish_bank_state(
    node_id: u32,
    bank:    &ParameterBank,
    buf:     &mut Vec<(String, StateBusValue)>,
) {
    for (name, value) in bank.iter_values() {
        buf.push((format!("/node/{}/{}", node_id, name), StateBusValue::Float(value)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::CapabilityDocument;
    use crate::command::{NodeCommand, CMD_BUMP_PARAM, CMD_SET_PARAM};

    fn make_doc() -> CapabilityDocument {
        use crate::capability::{ParamDescriptor, ParamUnit};
        use crate::port::{PortDescriptor, PortDirection, PortType};
        CapabilityDocument {
            name: "Test",
            vendor: "Test",
            version: (0, 1, 0),
            ports: vec![PortDescriptor {
                id: 0,
                name: "out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Audio,
            }],
            params: vec![
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("cutoff_hz"),
                    name: "cutoff_hz".into(),
                    min: 20.0,
                    max: 20000.0,
                    default: 1000.0,
                    stepped: false,
                    unit: ParamUnit::Hz,
                    display: None,
                },
                ParamDescriptor {
                    id: ParamDescriptor::id_for_name("resonance"),
                    name: "resonance".into(),
                    min: 0.1,
                    max: 4.0,
                    default: 0.7,
                    stepped: false,
                    unit: ParamUnit::Generic,
                    display: None,
                },
            ],
            extensions: vec![],
        }
    }

    fn cutoff_id() -> u32 { crate::capability::ParamDescriptor::id_for_name("cutoff_hz") }
    fn res_id()    -> u32 { crate::capability::ParamDescriptor::id_for_name("resonance") }

    fn cmd(type_id: u32, param_id: u32, value: f64) -> NodeCommand {
        NodeCommand { target_id: 0, type_id, arg0: param_id as i64, arg1: value }
    }

    #[test]
    fn parameter_bank_default_values() {
        let bank = ParameterBank::from_capability_document(&make_doc());
        assert_eq!(bank.get(cutoff_id()), 1000.0);
        assert_eq!(bank.get(res_id()),    0.7);
    }

    #[test]
    fn cmd_set_param_clamps_to_declared_range() {
        let mut bank = ParameterBank::from_capability_document(&make_doc());
        bank.handle_commands(&[cmd(CMD_SET_PARAM, cutoff_id(), 50000.0)]);
        assert_eq!(bank.get(cutoff_id()), 20000.0); // clamped to max
        bank.handle_commands(&[cmd(CMD_SET_PARAM, cutoff_id(), -100.0)]);
        assert_eq!(bank.get(cutoff_id()), 20.0); // clamped to min
    }

    #[test]
    fn cmd_bump_param_applies_delta_and_clamps() {
        let mut bank = ParameterBank::from_capability_document(&make_doc());
        bank.handle_commands(&[cmd(CMD_BUMP_PARAM, res_id(), 1.0)]);
        assert!((bank.get(res_id()) - 1.7).abs() < 1e-9);
        bank.handle_commands(&[cmd(CMD_BUMP_PARAM, res_id(), 100.0)]);
        assert_eq!(bank.get(res_id()), 4.0); // clamped to max
    }

    #[test]
    fn handle_commands_unknown_type_id_silently_ignored() {
        let mut bank = ParameterBank::from_capability_document(&make_doc());
        bank.handle_commands(&[NodeCommand { target_id: 0, type_id: 99, arg0: cutoff_id() as i64, arg1: 500.0 }]);
        assert_eq!(bank.get(cutoff_id()), 1000.0); // unchanged
    }

    #[test]
    fn get_returns_zero_for_unknown_param_id() {
        let bank = ParameterBank::from_capability_document(&make_doc());
        assert_eq!(bank.get(999_999), 0.0);
    }

    #[test]
    fn reset_restores_defaults() {
        let mut bank = ParameterBank::from_capability_document(&make_doc());
        bank.handle_commands(&[cmd(CMD_SET_PARAM, cutoff_id(), 5000.0)]);
        bank.reset();
        assert_eq!(bank.get(cutoff_id()), 1000.0);
    }

    fn make_single_param_doc(name: &'static str, default: f64) -> CapabilityDocument {
        use crate::capability::{ParamDescriptor, ParamUnit};
        use crate::port::{PortDescriptor, PortDirection, PortType};
        CapabilityDocument {
            name: "Test", vendor: "Test", version: (0, 1, 0),
            ports: vec![PortDescriptor { id: 0, name: "out".into(), direction: PortDirection::Output, port_type: PortType::Audio }],
            params: vec![
                ParamDescriptor { id: ParamDescriptor::id_for_name(name), name: name.into(), min: 0.0, max: 1.0, default, stepped: false, unit: ParamUnit::Generic, display: None },
            ],
            extensions: vec![],
        }
    }

    #[test]
    fn publish_bank_state_single_param() {
        let doc = make_single_param_doc("cutoff", 0.7);
        let bank = ParameterBank::from_capability_document(&doc);
        let mut buf: Vec<(String, StateBusValue)> = Vec::new();
        publish_bank_state(42, &bank, &mut buf);
        assert_eq!(buf.len(), 1);
        assert_eq!(buf[0].0, "/node/42/cutoff");
        assert_eq!(buf[0].1, StateBusValue::Float(0.7));
    }

    #[test]
    fn publish_bank_state_multi_param() {
        // make_doc() has 2 params (cutoff_hz + resonance) → 2 entries in buf
        let bank = ParameterBank::from_capability_document(&make_doc());
        let mut buf: Vec<(String, StateBusValue)> = Vec::new();
        publish_bank_state(7, &bank, &mut buf);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn publish_bank_state_empty_bank() {
        let bank = ParameterBank::empty();
        let mut buf: Vec<(String, StateBusValue)> = Vec::new();
        publish_bank_state(1, &bank, &mut buf);
        assert!(buf.is_empty());
    }

    #[test]
    fn parameter_slot_name_stored() {
        let doc = make_single_param_doc("resonance", 0.5);
        let bank = ParameterBank::from_capability_document(&doc);
        let pairs: Vec<_> = bank.iter_values().collect();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "resonance");
        assert!((pairs[0].1 - 0.5).abs() < 1e-12);
    }

    #[test]
    fn iter_values_reflects_mutation() {
        use crate::capability::ParamDescriptor;
        let doc = make_single_param_doc("resonance", 0.5);
        let res_id = ParamDescriptor::id_for_name("resonance");
        let mut bank = ParameterBank::from_capability_document(&doc);
        bank.handle_commands(&[cmd(CMD_SET_PARAM, res_id, 0.9)]);
        let pairs: Vec<_> = bank.iter_values().collect();
        assert!((pairs[0].1 - 0.9).abs() < 1e-12, "iter_values should reflect mutated value");
    }
}
