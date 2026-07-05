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
pub mod command;
pub mod constants;
pub mod context;
pub mod event;
pub mod surface;
pub mod midi;
pub mod node;
pub mod parameter;
pub mod port;
pub mod state_bus;
pub mod tempo_source;
pub mod templates;
pub mod transport;

pub use constants::TICKS_PER_BEAT;

pub use buffer::{
    AsSignal, AsSignalMut, AudioBuffer, CvBuffer, LogicBuffer, ModBuffer, PhaseBuffer,
    PitchBuffer,
};

pub use port::{PortDescriptor, PortDirection, PortName, PortType};

pub use event::{
    Event, ExtendedEventSlab, ParamLockEvent, ExtendedEventRef, TimedEvent, UmpMessage,
};

pub use surface::{
    ButtonDescriptor, Control, DisplayContent, DisplayDescriptor, DisplayType,
    EncoderBehaviour, EncoderDescriptor, FaderDescriptor, Surface, SurfaceEvent,
    SurfaceEventMsg, SurfaceOutput, SurfaceOutputHandle, LedDescriptor, LedUpdate,
    PadDescriptor, RgbColor, SurfaceDescriptor,
};

pub use transport::{TransportEvent, TransportFlags, TransportInfo};

pub use tempo_source::{ClockPriority, TempoSource};

pub use state_bus::{StateBusHandle, StateBusSubscription, StateBusValue, StatePublisher};

pub use context::{
    EventOutputBuffer, ProcessInput, ProcessOutput, SignalInputSlot, SignalOutputSlot,
    SignalPortKind,
};

pub use capability::{
    CapabilityDocument, ParamDescriptor, ParamDisplay, ParamDisplayAdapter, ParamUnit,
};

pub use agreement::{ConnectionAgreement, ConnectionRecord, LockableParam};

pub use node::{GraphNode, Negotiable, Node};

pub use templates::{
    ControllerNode, InstrumentNode, SequencerNode, SignalNode,
};

pub use command::{NodeCommand, CMD_BUMP_PARAM, CMD_SET_PARAM};

pub use parameter::{publish_bank_state, ParameterBank};
