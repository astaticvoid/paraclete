use crate::node::Node;
use crate::port::PortName;

// ── Hardware events ───────────────────────────────────────────────────────────

/// Native hardware control event. `Copy` — lives in pre-allocated event slices.
///
/// Carries typed, controller-specific information (grid position, velocity)
/// that cannot be cleanly represented in MIDI 2.0 UMP. A `HardwareMappingNode`
/// in the graph translates `Hardware` events to `Midi2` events for sound nodes.
#[derive(Clone, Copy, Debug)]
pub enum HardwareEvent {
    /// Pad pressed. velocity and pressure are 16-bit (MIDI 2.0 resolution).
    PadPressed { id: u32, velocity: u16, pressure: u16 },
    /// Pad released.
    PadReleased { id: u32 },
    /// Continuous pressure change on a held pad.
    PadPressure { id: u32, pressure: u16 },
    /// Button pressed (momentary).
    ButtonPressed { id: u32 },
    /// Button released.
    ButtonReleased { id: u32 },
    /// Encoder rotated. delta is signed — negative = counter-clockwise.
    EncoderDelta { id: u32, delta: i32 },
    /// Encoder shaft pressed or released.
    EncoderPush { id: u32, pressed: bool },
    /// Fader moved. value is 16-bit.
    FaderMoved { id: u32, value: u16 },
}

// ── Surface descriptor ────────────────────────────────────────────────────────

/// Complete description of a hardware controller's physical surface.
/// Allocated once at controller construction time; referenced by the runtime
/// for capability discovery.
pub struct SurfaceDescriptor {
    pub name: &'static str,
    pub vendor: &'static str,
    pub controls: Vec<Control>,
}

/// A single physical control on a hardware surface.
pub enum Control {
    Pad(PadDescriptor),
    Button(ButtonDescriptor),
    Encoder(EncoderDescriptor),
    Fader(FaderDescriptor),
    Led(LedDescriptor),
    Display(DisplayDescriptor),
}

/// A velocity/pressure-sensitive pad. Common on grid controllers.
pub struct PadDescriptor {
    pub id: u32,
    pub name: PortName,
    /// Position in a grid layout. `None` if not part of a grid.
    pub row: Option<u8>,
    pub col: Option<u8>,
    pub velocity_sensitive: bool,
    pub pressure_sensitive: bool,
    pub rgb: bool,
}

/// A momentary button.
pub struct ButtonDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// A rotary encoder.
pub struct EncoderDescriptor {
    pub id: u32,
    pub name: PortName,
    /// True if the encoder shaft can be pressed.
    pub has_push: bool,
    /// True for endless rotation. False for ranged (with min/max stops).
    pub endless: bool,
}

/// A linear fader.
pub struct FaderDescriptor {
    pub id: u32,
    pub name: PortName,
    pub motorised: bool,
}

/// A standalone LED (not part of a pad or button).
pub struct LedDescriptor {
    pub id: u32,
    pub name: PortName,
    pub rgb: bool,
}

/// A text or pixel display.
pub struct DisplayDescriptor {
    pub id: u32,
    pub name: PortName,
    pub cols: u8,
    pub rows: u8,
    pub display_type: DisplayType,
}

/// The rendering capability of a display.
pub enum DisplayType {
    /// 7-segment numeric display.
    Segment,
    /// Character LCD — fixed-width character grid.
    Character,
    /// Bitmap pixel display.
    Pixel,
}

// ── Hardware output ───────────────────────────────────────────────────────────

/// Collected output state sent to a hardware device after each cycle.
///
/// Stub at P1 — populated by the runtime at P2 when the state bus wires up
/// LED feedback. Until then, `update_output()` always receives an empty struct.
pub struct HardwareOutput {
    pub led_updates: Vec<LedUpdate>,
    pub display_updates: Vec<DisplayUpdate>,
}

impl HardwareOutput {
    pub fn empty() -> Self {
        Self { led_updates: vec![], display_updates: vec![] }
    }
}

pub struct LedUpdate {
    pub control_id: u32,
    pub color: RgbColor,
}

pub struct DisplayUpdate {
    pub control_id: u32,
    pub content: DisplayContent,
}

pub enum DisplayContent {
    /// UTF-8 text string.
    Text(String),
    /// Bitmask for 7-segment display.
    Segments(u8),
    /// Raw pixel bitmap. Format is device-defined.
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
    pub const OFF:   RgbColor = RgbColor { r: 0,   g: 0,   b: 0   };
    pub const WHITE: RgbColor = RgbColor { r: 255, g: 255, b: 255 };
    pub const RED:   RgbColor = RgbColor { r: 255, g: 0,   b: 0   };
    pub const GREEN: RgbColor = RgbColor { r: 0,   g: 255, b: 0   };
    pub const BLUE:  RgbColor = RgbColor { r: 0,   g: 0,   b: 255 };
}

// ── HardwareDevice trait ──────────────────────────────────────────────────────

/// A hardware controller node.
///
/// Implements `Node` for graph participation — the controller emits
/// `HardwareEvent` instances via `output.events_out` in `process()`.
///
/// Implements `HardwareDevice` to declare its physical surface and receive
/// output state (LED colours, display content) from the runtime.
pub trait HardwareDevice: Node {
    /// Declare the physical surface of this controller.
    /// Called once at registration time. May be called from any thread.
    fn surface(&self) -> &SurfaceDescriptor;

    /// Receive output state from the graph after each process cycle.
    ///
    /// Called on the audio thread immediately after `process()` completes
    /// for all nodes. At P1, `output` is always empty — LED feedback is
    /// wired at P2 when the state bus is available.
    fn update_output(&mut self, output: &HardwareOutput);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_event_is_copy() {
        let a = HardwareEvent::PadPressed { id: 0, velocity: 100, pressure: 0 };
        let b = a; // Copy
        let _ = a;
        let _ = b;
    }

    #[test]
    fn rgb_color_constants_have_correct_values() {
        assert_eq!(RgbColor::OFF,   RgbColor { r: 0,   g: 0,   b: 0   });
        assert_eq!(RgbColor::WHITE, RgbColor { r: 255, g: 255, b: 255 });
        assert_eq!(RgbColor::RED,   RgbColor { r: 255, g: 0,   b: 0   });
        assert_eq!(RgbColor::GREEN, RgbColor { r: 0,   g: 255, b: 0   });
        assert_eq!(RgbColor::BLUE,  RgbColor { r: 0,   g: 0,   b: 255 });
    }

    #[test]
    fn hardware_output_empty_has_no_updates() {
        let out = HardwareOutput::empty();
        assert!(out.led_updates.is_empty());
        assert!(out.display_updates.is_empty());
    }

    #[test]
    fn surface_descriptor_construction_for_test_surface() {
        let surface = SurfaceDescriptor {
            name: "Test Controller",
            vendor: "Test",
            controls: vec![
                Control::Pad(PadDescriptor {
                    id: 0,
                    name: "pad_0".into(),
                    row: Some(0),
                    col: Some(0),
                    velocity_sensitive: true,
                    pressure_sensitive: false,
                    rgb: true,
                }),
            ],
        };
        assert_eq!(surface.name, "Test Controller");
        assert_eq!(surface.controls.len(), 1);
    }
}
