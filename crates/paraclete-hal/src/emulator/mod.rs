mod surface;
mod terminal;

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal as ct;

use paraclete_node_api::{
    Event, HardwareDevice, HardwareEvent, HardwareOutput, Node, PortDescriptor, PortDirection,
    PortType, ProcessInput, ProcessOutput, SurfaceDescriptor, TimedEvent,
};

const RENDER_INTERVAL: Duration = Duration::from_millis(16);
// Fixed velocity for keyboard-triggered pads (MIDI 2.0 16-bit range, ~50% of max).
const KEY_VELOCITY: u16 = 32768;

/// A software simulation of the Novation Launchpad surface.
///
/// Implements `Node` (emitting `HardwareEvent`s into the graph) and
/// `HardwareDevice` (declaring its surface, receiving LED feedback at P2).
///
/// Terminal input is polled non-blocking in `process()` on the audio thread.
/// Raw mode is enabled in `activate()` (main thread) and restored in `deactivate()`.
/// Rendering is debounced to at most once per 16 ms.
pub struct LaunchpadEmulator {
    surface: SurfaceDescriptor,
    ports: [PortDescriptor; 1],
    /// Currently pressed pad ids — used for terminal rendering.
    pressed: HashSet<u32>,
    /// Events buffered between poll_keyboard() and process() drain.
    pending: Vec<HardwareEvent>,
    last_render: Option<Instant>,
    raw_mode_active: bool,
}

impl LaunchpadEmulator {
    pub fn new() -> Self {
        Self {
            surface: surface::build_launchpad_surface(),
            ports: [PortDescriptor {
                id: 0,
                name: "events_out".into(),
                direction: PortDirection::Output,
                port_type: PortType::Event,
            }],
            pressed: HashSet::new(),
            pending: Vec::new(),
            last_render: None,
            raw_mode_active: false,
        }
    }

    /// Non-blocking poll of crossterm keyboard events.
    fn poll_keyboard(&mut self) {
        loop {
            match crossterm::event::poll(Duration::ZERO) {
                Ok(true) => {}
                _ => break,
            }
            let Ok(event) = crossterm::event::read() else { break };
            match event {
                // Esc or Ctrl-C: restore terminal then exit.
                // In raw mode Ctrl-C is a raw key event, not SIGINT.
                crossterm::event::Event::Key(KeyEvent {
                    code: KeyCode::Esc,
                    kind: KeyEventKind::Press,
                    ..
                })
                | crossterm::event::Event::Key(KeyEvent {
                    code: KeyCode::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.deactivate();
                    std::process::exit(0);
                }

                crossterm::event::Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    if let Some(pad_id) = terminal::key_to_pad(code) {
                        if self.pressed.insert(pad_id) {
                            self.pending.push(HardwareEvent::PadPressed {
                                id: pad_id,
                                velocity: KEY_VELOCITY,
                                pressure: 0,
                            });
                        }
                    }
                }

                crossterm::event::Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Release,
                    ..
                }) => {
                    if let Some(pad_id) = terminal::key_to_pad(code) {
                        if self.pressed.remove(&pad_id) {
                            self.pending.push(HardwareEvent::PadReleased { id: pad_id });
                        }
                    }
                }

                _ => {}
            }
        }
    }

    fn maybe_render(&mut self) {
        let now = Instant::now();
        let should_render =
            self.last_render.map_or(true, |t| now.duration_since(t) >= RENDER_INTERVAL);
        if should_render {
            terminal::render(&self.pressed);
            self.last_render = Some(now);
        }
    }

}

impl Default for LaunchpadEmulator {
    fn default() -> Self { Self::new() }
}

impl Node for LaunchpadEmulator {
    fn ports(&self) -> &[PortDescriptor] {
        &self.ports
    }

    fn activate(&mut self, _sample_rate: f32, _block_size: usize) {
        if ct::enable_raw_mode().is_ok() {
            self.raw_mode_active = true;
            // Request key-release events via the kitty keyboard protocol.
            // Supported by iTerm2, WezTerm, Alacritty, Ghostty, Kitty.
            // Terminals that don't support it silently ignore the sequence;
            // in that case keys will sustain until the next key is pressed.
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PushKeyboardEnhancementFlags(
                    crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES,
                ),
                ct::Clear(ct::ClearType::All),
                crossterm::cursor::MoveTo(0, 0),
                crossterm::cursor::Hide,
            );
        }
    }

    fn deactivate(&mut self) {
        if self.raw_mode_active {
            let _ = crossterm::execute!(
                std::io::stdout(),
                crossterm::event::PopKeyboardEnhancementFlags,
                crossterm::cursor::Show,
                crossterm::cursor::MoveTo(0, 10),
            );
            let _ = ct::disable_raw_mode();
            self.raw_mode_active = false;
        }
    }

    fn process(&mut self, _input: &ProcessInput, output: &mut ProcessOutput) {
        self.poll_keyboard();
        self.maybe_render();

        for event in self.pending.drain(..) {
            output.events_out.push(TimedEvent::new(0, Event::Hardware(event)));
        }
    }
}

impl HardwareDevice for LaunchpadEmulator {
    fn surface(&self) -> &SurfaceDescriptor {
        &self.surface
    }

    fn update_output(&mut self, _output: &HardwareOutput) {
        // LED state update — wired at P2 when the state bus is available.
    }
}
