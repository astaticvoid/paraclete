mod surface;
mod terminal;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use self::terminal::{key_to_target, KeyTarget, CONTROL_BASE, SCENE_BASE};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::terminal as ct;

use paraclete_node_api::{
    Event, Node, PortDescriptor, PortDirection, PortType, ProcessInput, ProcessOutput, Surface,
    SurfaceDescriptor, SurfaceEvent, SurfaceOutput, TimedEvent,
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
/// Terminal input (INFRA-008: crossterm I/O must never run on the audio
/// thread) is read on a dedicated background thread spawned in
/// `activate()`, which pushes raw `crossterm::event::Event`s into
/// `input_queue`. `process()` only ever does a non-blocking `try_lock` +
/// drain — mirrors `LaunchpadNode`'s `incoming` pattern for MIDI input
/// (a callback thread owned by the MIDI library feeds a `Mutex<VecDeque>`;
/// here the thread is our own, since crossterm has no callback API, but
/// the audio-thread-side contract is identical).
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
    /// Events buffered between event handling and process() drain.
    pending: Vec<SurfaceEvent>,
    last_render: Option<Instant>,
    raw_mode_active: bool,
    /// Raw terminal events read by the background poll thread; drained
    /// non-blockingly in `process()` (INFRA-008).
    input_queue: Arc<Mutex<VecDeque<crossterm::event::Event>>>,
    /// Signals the background poll thread to exit. Cleared in
    /// `deactivate()`; the thread checks it once per poll timeout.
    running: Arc<AtomicBool>,
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
            input_queue: Arc::new(Mutex::new(VecDeque::new())),
            running: Arc::new(AtomicBool::new(false)),
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

    /// Handle one terminal event. Pure application logic — no I/O — so it
    /// stays safe to run on the audio thread; only the reading of events
    /// (`spawn_poll_thread`) had to move off it (INFRA-008).
    fn handle_event(&mut self, event: crossterm::event::Event) {
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

    /// Drain events the background poll thread has queued so far — a
    /// non-blocking `try_lock`, never a blocking `lock` (INFRA-008: this
    /// runs on the audio thread from `process()`, which must never block).
    /// A held lock (the poll thread mid-push) just means this cycle sees
    /// nothing new; the next cycle catches up.
    fn drain_input_queue(&mut self) {
        // Hard constraint 1 (AGENTS.md): process() must never allocate.
        // Popping one event per lock acquisition — rather than collecting
        // into an intermediate Vec — avoids that: `handle_event` needs
        // `&mut self`, which conflicts with holding the `input_queue`
        // guard for the whole drain, so each iteration re-locks, takes
        // one event, and drops the guard before calling it. `pop_front`
        // itself never allocates.
        loop {
            let event = match self.input_queue.try_lock() {
                Ok(mut q) => q.pop_front(),
                Err(_) => return,
            };
            match event {
                Some(event) => self.handle_event(event),
                None => return,
            }
        }
    }

    /// Spawn the background thread that owns all crossterm I/O
    /// (INFRA-008). Blocking `poll`/`read` are fine here — this is not
    /// the audio thread. Exits once `running` is cleared (`deactivate()`);
    /// bounded by one `poll` timeout, so shutdown is prompt but not
    /// instantaneous.
    fn spawn_poll_thread(&mut self) {
        self.running.store(true, Ordering::SeqCst);
        let running = Arc::clone(&self.running);
        let queue = Arc::clone(&self.input_queue);
        std::thread::spawn(move || {
            while running.load(Ordering::Relaxed) {
                match crossterm::event::poll(Duration::from_millis(30)) {
                    Ok(true) => {
                        if let Ok(event) = crossterm::event::read() {
                            if let Ok(mut q) = queue.lock() {
                                q.push_back(event);
                            }
                        }
                    }
                    Ok(false) => {}
                    Err(_) => break,
                }
            }
        });
    }

    fn maybe_render(&mut self) {
        let now = Instant::now();
        let should_render = self
            .last_render
            .map_or(true, |t| now.duration_since(t) >= RENDER_INTERVAL);
        if should_render {
            terminal::render(self.active_row, &self.pressed, self.mode.label());
            self.last_render = Some(now);
        }
    }
}

impl Default for LaunchpadEmulator {
    fn default() -> Self {
        Self::new()
    }
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
            // INFRA-008: raw mode must be enabled before the poll thread
            // starts reading, so start it last.
            self.spawn_poll_thread();
        }
    }

    fn deactivate(&mut self) {
        // INFRA-008: stop the poll thread first — it reads via crossterm,
        // which needs the terminal still in raw mode to decode correctly.
        self.running.store(false, Ordering::SeqCst);
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
        self.drain_input_queue();
        self.maybe_render();

        for event in self.pending.drain(..) {
            output
                .events_out
                .push(TimedEvent::new(0, Event::Surface(event)));
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

    /// INFRA-008: `process()` no longer reads crossterm directly — it
    /// drains `input_queue`, which the background poll thread fills. This
    /// pins the queue → `handle_event` plumbing without spawning a real
    /// thread or touching the terminal: push a raw event exactly as the
    /// poll thread would, then drain.
    #[test]
    fn drain_input_queue_applies_queued_key_events() {
        let mut emu = LaunchpadEmulator::new();
        let press =
            crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        emu.input_queue.lock().unwrap().push_back(press);

        emu.drain_input_queue();

        assert!(
            matches!(
                emu.pending.as_slice(),
                [SurfaceEvent::PadPressed { id: 0, .. }]
            ),
            "queued Press('q') must resolve to the same PadPressed(0) \
             apply_press would produce directly; got: {:?}",
            emu.pending
        );
    }

    /// A queue drain must not leave anything behind for the next cycle to
    /// re-apply — otherwise a step would toggle twice.
    #[test]
    fn drain_input_queue_empties_the_queue() {
        let mut emu = LaunchpadEmulator::new();
        let press =
            crossterm::event::Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        emu.input_queue.lock().unwrap().push_back(press);

        emu.drain_input_queue();

        assert!(
            emu.input_queue.lock().unwrap().is_empty(),
            "drain must consume every queued event, not leave a residue"
        );
    }

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
