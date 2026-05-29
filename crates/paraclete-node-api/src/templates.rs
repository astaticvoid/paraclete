/// Convenience super-traits for common node archetypes.
/// These are the primary contribution path for third-party developers.
/// Signatures only at P0 — blanket `Node` impls ship alongside the first real nodes.
use crate::buffer::AudioBuffer;
use crate::context::{ProcessInput, ProcessOutput};
use crate::event::UmpMessage;
use crate::node::Node;
use crate::transport::TransportInfo;

/// For pure signal processors: oscillators, filters, effects, math modules.
/// Implement `dsp()` with your processing logic; the blanket `Node` impl
/// handles buffer routing boilerplate.
pub trait SignalNode: Node {
    fn dsp(&mut self, input: &ProcessInput, output: &mut ProcessOutput);
}

/// For MIDI-driven instruments: synths, samplers.
pub trait InstrumentNode: Node {
    fn on_note_on(&mut self, packet: &UmpMessage);
    fn on_note_off(&mut self, packet: &UmpMessage);
    fn render(&mut self, outputs: &mut [&mut AudioBuffer]);
}

/// For event-generating sequencer nodes.
pub trait SequencerNode: Node {
    fn on_clock_tick(&mut self, transport: &TransportInfo);
    fn emit_events(&mut self, output: &mut ProcessOutput);
}

/// For hardware controller nodes.
pub trait ControllerNode: Node {
    /// Called when a UMP packet arrives from physical hardware.
    fn on_packet(&mut self, packet: &UmpMessage);
    /// Set an LED on the controller surface (r, g, b = 0–255).
    fn set_led(&mut self, index: u8, r: u8, g: u8, b: u8);
}
