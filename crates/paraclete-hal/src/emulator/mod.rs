mod surface;
mod terminal;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use self::terminal::{key_to_target, KeyTarget, CONTROL_BASE, SCENE_BASE};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal as ct;

use paraclete_node_api::{
    Event, Surface, SurfaceEvent, SurfaceOutput, Node, PortDescriptor, PortDirection,
    PortType, ProcessInput, ProcessOutput, SurfaceDescriptor, TimedEvent,
};

const RENDER_INTERVAL: Duration = Duration::from_millis(16);
// Fixed velocity for keyboard-triggered pads (MIDI 2.0 16-bit range, ~50% of max).
const KEY_VELOCITY: u16 = 32768;

/// Emulator input mode. `Tab` cycles. Grid is the only active mode at P9.5
/// Commit 1; Encoder/Piano are wired in later commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmuMode {
    Grid,
    Encoder,
    Piano,
}

impl EmuMode {
    fn label(self) -> &'static str {
        match self {
            EmuMode::Grid => "GRID",
            EmuMode::Encoder => "ENC",
            EmuMode::Piano => "PIANO",
        }
    }
    fn next(self) -> Self {
        match self {
            EmuMode::Grid => EmuMode::Encoder,
            EmuMode::Encoder => EmuMode::Piano,
            EmuMode::Piano => EmuMode::Grid,
        }
    }
}

/// A software simulation of the Novation Launchpad surface.
///
/// Implements `Node` (emitting `SurfaceEvent`s into the graph) and
/// `Surface` (declaring its surface, receiving LED feedback at P2).
///
/// Terminal input is polled non-blocking in `process()` on the audio thread.
/// Raw mode is enabled in `activate()` (main thread) and restored in `deactivate()`.
/// Rendering is debounced to at most once per 16 ms.
///
/// Keyboard scheme (Grid mode): `1`–`8` select the active track row;
/// `Q W E R T Y U I` toggle the 8 step pads in that row; `A S D F G H J K` are
/// the scene buttons; `Z X C V B N M ,` are the top control row.
pub struct LaunchpadEmulator {
    surface: SurfaceDescriptor,
    ports: [PortDescriptor; 1],
    /// Active track row (0–7) that the step keys edit.
    active_row: u8,
    /// Current input mode.
    mode: EmuMode,
    /// Keys currently held → the control id emitted on their press. Lets a
    /// release emit the correct id even if `active_row` changed meanwhile.
    held: HashMap<KeyCode, u32>,
    /// Currently pressed control ids — used for terminal rendering.
    pressed: HashSet<u32>,
    /// Events buffered between poll_keyboard() and process() drain.
    pending: Vec<SurfaceEvent>,
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
            active_row: 0,
            mode: EmuMode::Grid,
            held: HashMap::new(),
            pressed: HashSet::new(),
            pending: Vec::new(),
            last_render: None,
            raw_mode_active: false,
        }
    }

    /// Apply a key-press target against the current `active_row`, buffering the
    /// resulting `SurfaceEvent` and tracking the held id for release.
    fn apply_press(&mut self, code: KeyCode, target: KeyTarget) {
        let id = match target {
            // Cursor move only — no event, no held entry.
            KeyTarget::RowSelect(r) => {
                self.active_row = r;
                return;
            }
            KeyTarget::Step(col) => self.active_row as u32 * 8 + col as u32,
            KeyTarget::Scene(n) => SCENE_BASE + n as u32,
            KeyTarget::Control(n) => CONTROL_BASE + n as u32,
        };
        // Ignore duplicate presses of an already-held key (auto-repeat, or a
        // re-press in terminals without key-release reporting). The `Vacant`
        // guard must not overwrite the held id — a later release relies on the
        // original.
        if let std::collections::hash_map::Entry::Vacant(slot) = self.held.entry(code) {
            slot.insert(id);
            self.pressed.insert(id);
            // BUG-014: scene (64-71) and control (72-79) ids are buttons, not pads.
            if id >= SCENE_BASE {
                self.pending.push(SurfaceEvent::ButtonPressed { id });
            } else {
                self.pending.push(SurfaceEvent::PadPressed {
                    id,
                    velocity: KEY_VELOCITY,
                    pressure: 0,
                });
            }
        }
    }

    /// Release whatever control id the given key was holding (if any).
    fn apply_release(&mut self, code: KeyCode) {
        if let Some(id) = self.held.remove(&code) {
            self.pressed.remove(&id);
            if id >= SCENE_BASE {
                self.pending.push(SurfaceEvent::ButtonReleased { id });
            } else {
                self.pending.push(SurfaceEvent::PadReleased { id });
            }
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

                // Tab cycles the input mode.
                crossterm::event::Event::Key(KeyEvent {
                    code: KeyCode::Tab,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    self.mode = self.mode.next();
                }

                crossterm::event::Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Press,
                    ..
                }) => {
                    // Grid is the only active mode at Commit 1.
                    if self.mode == EmuMode::Grid {
                        if let Some(target) = key_to_target(code) {
                            self.apply_press(code, target);
                        }
                    }
                }

                crossterm::event::Event::Key(KeyEvent {
                    code,
                    kind: KeyEventKind::Release,
                    ..
                }) => {
                    // Release regardless of current mode — a key held from Grid
                    // mode must release even if the mode changed meanwhile.
                    self.apply_release(code);
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
            terminal::render(self.active_row, &self.pressed, self.mode.label());
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
            output.events_out.push(TimedEvent::new(0, Event::Surface(event)));
        }
    }
}

impl Surface for LaunchpadEmulator {
    fn descriptor(&self) -> &SurfaceDescriptor {
        &self.surface
    }

    fn update_output(&mut self, _output: &SurfaceOutput) {
        // LED state update — wired at P9.5 Commit 2.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paraclete_node_api::Control;

    #[test]
    fn surface_has_64_grid_8_scene_8_control() {
        let emu = LaunchpadEmulator::new();
        let pads = emu
            .surface
            .controls
            .iter()
            .filter(|c| matches!(c, Control::Pad(_)))
            .count();
        assert_eq!(pads, 80); // 64 grid + 8 scene + 8 control
    }

    #[test]
    fn step_id_uses_active_row() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('4'), KeyTarget::RowSelect(3));
        assert_eq!(emu.active_row, 3);
        emu.apply_press(KeyCode::Char('e'), KeyTarget::Step(2)); // row 3 col 2 = 26
        assert!(matches!(
            emu.pending.as_slice(),
            [SurfaceEvent::PadPressed { id: 26, .. }]
        ));
    }

    #[test]
    fn release_emits_pressed_id_after_row_change() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('1'), KeyTarget::RowSelect(1));
        emu.apply_press(KeyCode::Char('q'), KeyTarget::Step(0)); // row 1 col 0 = 8
        emu.pending.clear();
        emu.apply_press(KeyCode::Char('6'), KeyTarget::RowSelect(5)); // row changes
        emu.apply_release(KeyCode::Char('q'));
        assert!(matches!(
            emu.pending.as_slice(),
            [SurfaceEvent::PadReleased { id: 8 }]
        ));
    }

    #[test]
    fn scene_and_control_emit_button_events() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('a'), KeyTarget::Scene(0));
        emu.apply_press(KeyCode::Char('z'), KeyTarget::Control(0));
        let ids: Vec<u32> = emu
            .pending
            .iter()
            .filter_map(|e| match e {
                SurfaceEvent::ButtonPressed { id } => Some(*id),
                _ => None,
            })
            .collect();
        assert_eq!(ids, vec![SCENE_BASE, CONTROL_BASE]); // 64, 72
    }

    #[test]
    fn autorepeat_press_emits_once() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('q'), KeyTarget::Step(0));
        emu.apply_press(KeyCode::Char('q'), KeyTarget::Step(0)); // auto-repeat
        let presses = emu
            .pending
            .iter()
            .filter(|e| matches!(e, SurfaceEvent::PadPressed { .. }))
            .count();
        assert_eq!(presses, 1);
    }

    #[test]
    fn duplicate_press_does_not_rebind_held_id() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('q'), KeyTarget::Step(0)); // row 0 col 0 = id 0
        emu.apply_press(KeyCode::Char('5'), KeyTarget::RowSelect(4));
        emu.apply_press(KeyCode::Char('q'), KeyTarget::Step(0)); // re-press; row now 4
        emu.pending.clear();
        emu.apply_release(KeyCode::Char('q'));
        // Release must carry the ORIGINAL id (0), not the rebound row-4 id.
        assert!(matches!(
            emu.pending.as_slice(),
            [SurfaceEvent::PadReleased { id: 0 }]
        ));
    }

    #[test]
    fn row_select_emits_no_event() {
        let mut emu = LaunchpadEmulator::new();
        emu.apply_press(KeyCode::Char('5'), KeyTarget::RowSelect(4));
        assert!(emu.pending.is_empty());
        assert_eq!(emu.active_row, 4);
    }

    #[test]
    fn mode_cycles_grid_encoder_piano() {
        assert_eq!(EmuMode::Grid.next(), EmuMode::Encoder);
        assert_eq!(EmuMode::Encoder.next(), EmuMode::Piano);
        assert_eq!(EmuMode::Piano.next(), EmuMode::Grid);
    }
}
