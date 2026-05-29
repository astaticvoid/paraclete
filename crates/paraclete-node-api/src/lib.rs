// SPDX-License-Identifier: LGPL-3.0-or-later
//! Paraclete L2 Node API.
//!
//! This crate defines the universal contract every Paraclete node implements.
//! It is the only crate third-party node authors need to depend on.
//!
//! License: LGPL-3.0 — node implementations may remain closed-source.

pub mod agreement;
pub mod buffer;
pub mod capability;
pub mod constants;
pub mod context;
pub mod event;
pub mod hardware;
pub mod state_bus;
pub mod midi;
pub mod node;
pub mod port;
pub mod tempo_source;
pub mod templates;
pub mod transport;

// Flat re-exports for the common case.
pub use constants::TICKS_PER_BEAT;

pub use buffer::{
    AsSignal, AsSignalMut, AudioBuffer, CvBuffer, LogicBuffer, ModBuffer, PhaseBuffer,
    PitchBuffer,
};

pub use port::{PortDescriptor, PortDirection, PortName, PortType};

pub use event::{
    Event, ExtendedEventSlab, ParamLockEvent, ExtendedEventRef, TimedEvent, UmpMessage,
};

pub use hardware::{
    ButtonDescriptor, Control, DisplayContent, DisplayDescriptor, DisplayType, EncoderDescriptor,
    FaderDescriptor, HardwareDevice, HardwareEvent, HardwareOutput, LedDescriptor, LedUpdate,
    PadDescriptor, RgbColor, SurfaceDescriptor,
};

pub use transport::{TransportEvent, TransportFlags, TransportInfo};

pub use tempo_source::{ClockPriority, TempoSource};

pub use state_bus::{StateBusValue, StatePublisher};

pub use context::{
    EventOutputBuffer, ProcessInput, ProcessOutput, SignalInputSlot, SignalOutputSlot,
    SignalPortKind,
};

pub use capability::{
    CapabilityDocument, ParamDescriptor, ParamDisplay, ParamDisplayAdapter, ParamUnit,
};

pub use agreement::ConnectionAgreement;

pub use node::Node;

pub use templates::{
    ControllerNode, InstrumentNode, SequencerNode, SignalNode,
};
