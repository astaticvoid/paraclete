// SPDX-License-Identifier: GPL-3.0-or-later
//! `LoopBreakNode` — single-buffer feedback loop break (ADR-028).
//!
//! Introduces exactly one buffer of latency in a feedback path, allowing the
//! runtime to accept a cycle containing exactly one `LoopBreakNode`.
//!
//! - Port 0: `cv_in`  — CvSignal input
//! - Port 1: `cv_out` — CvSignal output
//!
//! Each cycle, the node:
//!   1. Captures the incoming `cv_in` signal into `next` (for the next cycle).
//!   2. Writes `prev` (the previous cycle's captured input) to `cv_out`.
//!
//! After all nodes have processed, the executor calls `loop_break_swap()` to
//! move `next` into `prev`, making it available via `loop_break_prev()` in the
//! following cycle. The pre-execution phase also writes `prev` into the upstream
//! signal buffer before downstream nodes process, so they receive the delayed
//! signal via normal signal routing.

use paraclete_node_api::{
    CapabilityDocument, Node, PortDescriptor, PortDirection, PortType,
    ProcessInput, ProcessOutput,
};

pub struct LoopBreakNode {
    ports: [PortDescriptor; 2],
    /// Data from the previous cycle — output to the downstream node.
    prev: Vec<f32>,
    /// Data captured from the current cycle's input — swapped into prev at end of cycle.
    next: Vec<f32>,
    block_size: usize,
}

impl LoopBreakNode {
    pub fn new() -> Self {
        LoopBreakNode {
            ports: [
                PortDescriptor {
                    id: 0,
                    name: "cv_in".into(),
                    direction: PortDirection::Input,
                    port_type: PortType::Cv,
                },
                PortDescriptor {
                    id: 1,
                    name: "cv_out".into(),
                    direction: PortDirection::Output,
                    port_type: PortType::Cv,
                },
            ],
            prev: Vec::new(),
            next: Vec::new(),
            block_size: 0,
        }
    }
}

impl Default for LoopBreakNode {
    fn default() -> Self { Self::new() }
}

impl Node for LoopBreakNode {
    fn ports(&self) -> &[PortDescriptor] { &self.ports }

    fn activate(&mut self, _sample_rate: f32, block_size: usize) {
        self.block_size = block_size;
        self.prev = vec![0.0_f32; block_size];
        self.next = vec![0.0_f32; block_size];
    }

    fn deactivate(&mut self) {
        self.prev.clear();
        self.next.clear();
        self.block_size = 0;
    }

    fn process(&mut self, input: &ProcessInput, output: &mut ProcessOutput) {
        // Capture this cycle's input for next cycle.
        let cv_in = input.cv_signal(0);
        if cv_in.len() >= self.block_size && self.block_size > 0 {
            self.next.copy_from_slice(&cv_in[..self.block_size]);
        }
        // Write prev (previous cycle's data) to output — consistent with pre-phase.
        if self.block_size > 0 {
            let cv_out = output.cv_signal_output_mut(1);
            cv_out.copy_from_slice(&self.prev);
        }
    }

    fn is_loop_break(&self) -> bool { true }

    fn loop_break_prev(&self) -> &[f32] { &self.prev }

    fn loop_break_swap(&mut self) {
        std::mem::swap(&mut self.prev, &mut self.next);
    }

    fn capability_document(&self) -> CapabilityDocument {
        CapabilityDocument {
            name: "LoopBreakNode".into(),
            vendor: "Paraclete".into(),
            version: (0, 1, 0),
            ports: self.ports.to_vec(),
            params: vec![],
            extensions: vec![],
        }
    }

    fn type_name(&self) -> &'static str { "LoopBreakNode" }
    fn serialize(&self) -> Vec<u8> { vec![] }
    fn deserialize(&mut self, _data: &[u8]) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::{
        EventOutputBuffer, ExtendedEventSlab,
        SignalInputSlot, SignalOutputSlot, SignalPortKind, TransportInfo,
    };

    fn make_lb_input<'a>(
        transport: &'a TransportInfo,
        slab: &'a ExtendedEventSlab,
        signal_inputs: &'a [SignalInputSlot],
        block_size: usize,
    ) -> ProcessInput<'a> {
        ProcessInput {
            audio_inputs: &[],
            signal_inputs,
            events: &[],
            transport,
            sample_rate: 44100.0,
            block_size,
            extended_events: slab,
            commands: &[],
        }
    }

    fn run_loop_break_cycle(
        lb: &mut LoopBreakNode,
        input_vals: &[f32],
    ) -> Vec<f32> {
        let block_size = input_vals.len();
        let transport = TransportInfo::default();
        let slab = ExtendedEventSlab::empty();

        // Set up cv input slot
        let in_slot = SignalInputSlot::new(0, SignalPortKind::Cv, input_vals);
        let signal_inputs = [in_slot];
        let input = make_lb_input(&transport, &slab, &signal_inputs, block_size);

        // Set up cv output slot
        let mut out_buf = vec![0.0f32; block_size];
        let mut events_out = EventOutputBuffer::new(16);
        let out_slot = SignalOutputSlot::new(1, SignalPortKind::Cv, &mut out_buf);
        let mut sig_outs = [out_slot];
        let mut output = ProcessOutput::new(
            &mut [],
            &mut sig_outs,
            &mut events_out,
        );

        lb.process(&input, &mut output);
        out_buf
    }

    #[test]
    fn loop_break_node_initial_output_zero() {
        let mut lb = LoopBreakNode::new();
        lb.activate(44100.0, 4);
        let input = [1.0f32, 1.0, 1.0, 1.0];
        let out = run_loop_break_cycle(&mut lb, &input);
        assert!(
            out.iter().all(|&s| s == 0.0),
            "first cycle output should be all zeros (prev starts zeroed), got {:?}",
            out
        );
    }

    #[test]
    fn loop_break_node_output_lags_one_cycle() {
        let mut lb = LoopBreakNode::new();
        lb.activate(44100.0, 4);

        // Cycle 1: input=[1,1,1,1], output=[0,0,0,0]
        let out1 = run_loop_break_cycle(&mut lb, &[1.0, 1.0, 1.0, 1.0]);
        assert_eq!(out1, vec![0.0, 0.0, 0.0, 0.0], "cycle 1 output should be zeros");
        lb.loop_break_swap();

        // Cycle 2: input=[2,2,2,2], output=[1,1,1,1]
        let out2 = run_loop_break_cycle(&mut lb, &[2.0, 2.0, 2.0, 2.0]);
        assert_eq!(out2, vec![1.0, 1.0, 1.0, 1.0], "cycle 2 output should be cycle 1 input");
        lb.loop_break_swap();

        // Cycle 3: input=[3,3,3,3], output=[2,2,2,2]
        let out3 = run_loop_break_cycle(&mut lb, &[3.0, 3.0, 3.0, 3.0]);
        assert_eq!(out3, vec![2.0, 2.0, 2.0, 2.0], "cycle 3 output should be cycle 2 input");
    }

    #[test]
    fn loop_break_node_is_loop_break_true() {
        let lb = LoopBreakNode::new();
        assert!(lb.is_loop_break(), "LoopBreakNode::is_loop_break() must return true");
    }

    #[test]
    fn non_loop_break_node_is_loop_break_false() {
        use crate::FilterNode;
        let f = FilterNode::new();
        assert!(!f.is_loop_break(), "FilterNode::is_loop_break() must return false");
    }
}
