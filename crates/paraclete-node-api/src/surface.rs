use std::borrow::Cow;

use crate::node::Node;
use crate::port::PortName;

// ── Hardware events ───────────────────────────────────────────────────────────

/// Native hardware control event. `Copy` — lives in pre-allocated event slices.
#[derive(Clone, Copy, Debug)]
pub enum SurfaceEvent {
    /// Pad pressed. velocity and pressure are 16-bit (MIDI 2.0 resolution).
    PadPressed {
        id: u32,
        velocity: u16,
        pressure: u16,
    },
    /// Pad released.
    PadReleased { id: u32 },
    /// Continuous pressure change on a held pad.
    PadPressure { id: u32, pressure: u16 },
    /// Button pressed (momentary).
    ButtonPressed { id: u32 },
    /// Button released.
    ButtonReleased { id: u32 },
    /// Encoder changed. delta is signed — negative = counter-clockwise.
    EncoderChanged { id: u32, value: u16, delta: i16 },
    /// Encoder shaft pressed or released.
    EncoderPush { id: u32, pressed: bool },
    /// Fader moved. value is 16-bit.
    FaderMoved { id: u32, value: u16 },
}

/// A hardware event tagged with the source device node id.
/// Produced by `ScriptingGatewayNode` and consumed by the scripting engine.
#[derive(Clone, Copy, Debug)]
pub struct SurfaceEventMsg {
    pub device_id: u32,
    pub event: SurfaceEvent,
}

// ── Surface descriptor ────────────────────────────────────────────────────────

/// Complete description of a hardware controller's physical surface.
#[derive(Clone, Debug)]
pub struct SurfaceDescriptor {
    /// `Cow` so fixed devices stay zero-alloc (`"Launchpad".into()`) while
    /// runtime-named surfaces — dynamic per-client Theoria, hot-plugged
    /// controllers — carry an owned name without leaking (U1 audit).
    pub name: Cow<'static, str>,
    pub vendor: Cow<'static, str>,
    pub controls: Vec<Control>,
}

/// A single physical control on a hardware surface.
#[derive(Clone, Debug)]
pub enum Control {
    Pad(PadDescriptor),
    Button(ButtonDescriptor),
    Encoder(EncoderDescriptor),
    Fader(FaderDescriptor),
    Led(LedDescriptor),
    Display(DisplayDescriptor),
}

/// A velocity/pressure-sensitive pad.
#[derive(Clone, Debug)]
pub struct PadDescriptor {
    pub id: u32,
    pub name: PortName,
    pub row: Option<u8>,
    pub col: Option<u8>,
    pub velocity_sensitive: bool,
    pub pressure_sensitive: bool,
    pub rgb: bool,
}

/// A momentary button.
#[derive(Clone, Debug)]
pub struct ButtonDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// Whether an encoder reports absolute position or relative deltas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EncoderBehaviour {
    /// Sends absolute position. `range_max` = 127 for standard 7-bit CC.
    Absolute { range_max: u16 },
    /// Sends signed delta. Positive = clockwise. Used by Digitakt and XL Mk3.
    Relative,
}

/// A rotary encoder.
#[derive(Clone, Debug)]
pub struct EncoderDescriptor {
    pub id: u32,
    pub name: PortName,
    pub has_push: bool,
    pub behaviour: EncoderBehaviour,
}

/// A linear fader.
#[derive(Clone, Debug)]
pub struct FaderDescriptor {
    pub id: u32,
    pub name: PortName,
    pub motorised: bool,
}

/// A standalone LED (not part of a pad or button).
#[derive(Clone, Debug)]
pub struct LedDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// A text or pixel display.
#[derive(Clone, Debug)]
pub struct DisplayDescriptor {
    pub id: u32,
    pub name: PortName,
    pub cols: u8,
    pub rows: u8,
    pub display_type: DisplayType,
}

#[derive(Clone, Debug)]
pub enum DisplayType {
    Segment,
    Character,
    Pixel,
}

// ── Hardware output ───────────────────────────────────────────────────────────

/// Collected output state sent to a hardware device after each cycle.
#[derive(Clone, Debug)]
pub struct SurfaceOutput {
    pub led_updates: Vec<LedUpdate>,
    pub display_updates: Vec<DisplayUpdate>,
}

impl SurfaceOutput {
    pub fn empty() -> Self {
        Self {
            led_updates: vec![],
            display_updates: vec![],
        }
    }
}

#[derive(Clone, Debug)]
pub struct LedUpdate {
    pub control_id: u32,
    pub color: RgbColor,
}

#[derive(Clone, Debug)]
pub struct DisplayUpdate {
    pub control_id: u32,
    pub content: DisplayContent,
}

#[derive(Clone, Debug)]
pub enum DisplayContent {
    Text(String),
    Segments(u8),
    Pixels(Vec<u8>),
}

/// An RGB colour value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RgbColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl RgbColor {
    pub const OFF: RgbColor = RgbColor { r: 0, g: 0, b: 0 };
    pub const WHITE: RgbColor = RgbColor {
        r: 255,
        g: 255,
        b: 255,
    };
    pub const RED: RgbColor = RgbColor { r: 255, g: 0, b: 0 };
    pub const GREEN: RgbColor = RgbColor { r: 0, g: 255, b: 0 };
    pub const BLUE: RgbColor = RgbColor { r: 0, g: 0, b: 255 };
}

// ── SurfaceOutputHandle ──────────────────────────────────────────────────────

/// Main-thread output handler for a hardware device.
///
/// Created by `Surface::take_output_handle()` at registration time.
/// The configurator holds it and calls `tick()` each main-loop iteration.
/// The device's audio-thread side pushes `SurfaceOutput` to an internal SPSC;
/// `tick()` drains and delivers to the physical device. Must not block.
pub trait SurfaceOutputHandle: Send {
    /// Called by the configurator each main-loop iteration. Drains the
    /// device's internal audio-thread SPSC and sends pending output.
    fn tick(&mut self);

    /// Deliver script-generated output directly from the main thread.
    /// Called after `process_subscriptions()` with LED updates from Rhai scripts.
    /// Default: no-op (device has no LED output from scripts).
    fn deliver(&mut self, _output: SurfaceOutput) {}
}

// ── Surface trait ──────────────────────────────────────────────────────

/// A hardware controller node. Implements `Node` for graph participation.
///
/// Two output paths are available:
/// - **Legacy path:** `update_output()` called from the executor (audio thread).
/// - **P4+ path:** implement `take_output_handle()` returning `Some(...)`. The
///   configurator holds the handle and calls `handle.tick()` each main-loop
///   iteration (main thread). Use this for all new hardware nodes.
pub trait Surface: Node {
    fn descriptor(&self) -> &SurfaceDescriptor;

    /// Legacy output path: called from the executor after each process cycle.
    /// New P4+ devices implement `take_output_handle()` instead and leave this
    /// as a no-op.
    fn update_output(&mut self, output: &SurfaceOutput);

    /// Called once by the configurator after registration. Return `Some` to opt
    /// into the main-thread output path; `None` uses the legacy audio-thread path.
    fn take_output_handle(&mut self) -> Option<Box<dyn SurfaceOutputHandle>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_event_is_copy() {
        let a = SurfaceEvent::PadPressed {
            id: 0,
            velocity: 100,
            pressure: 0,
        };
        let b = a;
        let _ = a;
        let _ = b;
    }

    #[test]
    fn surface_event_msg_is_copy() {
        let msg = SurfaceEventMsg {
            device_id: 1,
            event: SurfaceEvent::ButtonPressed { id: 5 },
        };
        let _ = msg;
        let _ = msg;
    }

    #[test]
    fn rgb_color_constants_have_correct_values() {
        assert_eq!(RgbColor::OFF, RgbColor { r: 0, g: 0, b: 0 });
        assert_eq!(
            RgbColor::WHITE,
            RgbColor {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(RgbColor::RED, RgbColor { r: 255, g: 0, b: 0 });
        assert_eq!(RgbColor::GREEN, RgbColor { r: 0, g: 255, b: 0 });
        assert_eq!(RgbColor::BLUE, RgbColor { r: 0, g: 0, b: 255 });
    }

    #[test]
    fn hardware_output_empty_has_no_updates() {
        let out = SurfaceOutput::empty();
        assert!(out.led_updates.is_empty());
        assert!(out.display_updates.is_empty());
    }

    #[test]
    fn encoder_behaviour_absolute_vs_relative() {
        let abs = EncoderBehaviour::Absolute { range_max: 127 };
        let rel = EncoderBehaviour::Relative;
        assert_ne!(abs, rel);
        assert_eq!(abs, EncoderBehaviour::Absolute { range_max: 127 });
    }

    #[test]
    fn hardware_output_handle_is_object_safe() {
        // Verify the trait is object-safe by forming a trait object reference.
        fn _takes_handle(_h: &dyn SurfaceOutputHandle) {}
    }

    #[test]
    fn surface_descriptor_carries_runtime_owned_name_without_leak() {
        // U1: a dynamic per-client Theoria surface / hot-plugged controller
        // carries an owned name instead of leaking a &'static str.
        let dynamic = format!("Theoria client {}", 7);
        let d = SurfaceDescriptor {
            name: dynamic.clone().into(),
            vendor: String::from("Runtime").into(),
            controls: vec![],
        };
        assert!(
            matches!(d.name, Cow::Owned(_)),
            "runtime surface name must be owned"
        );
        assert_eq!(d.name.as_ref(), dynamic.as_str());
    }

    #[test]
    fn surface_descriptor_construction() {
        let surface = SurfaceDescriptor {
            name: "Test Controller".into(),
            vendor: "Test".into(),
            controls: vec![Control::Pad(PadDescriptor {
                id: 0,
                name: "pad_0".into(),
                row: Some(0),
                col: Some(0),
                velocity_sensitive: true,
                pressure_sensitive: false,
                rgb: true,
            })],
        };
        assert_eq!(surface.name, "Test Controller");
        assert_eq!(surface.controls.len(), 1);
    }
}
