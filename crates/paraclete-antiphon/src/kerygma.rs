// SPDX-License-Identifier: GPL-3.0-or-later
//! kerygma — the outbound broadcast module.
//!
//! Owns the per-client outbound senders (`ClientTable`), the LED shadow table,
//! and the fan-out of `led` batches to every connected Theoria client. Runs on
//! the main thread only (called from `TheoriaOutputHandle::tick()`/`deliver()`);
//! WS write I/O happens on the per-client threads that own the `mpsc` receivers.
//!
//! W0 scope: LED fan-out + full-surface replay to new clients. W1 Commit 2
//! adds the state/context mirror as `AntiphonHandle::pump()` in `lib.rs`
//! (it needs no LED shadow, so it doesn't live here) — see `w1-interfaces.md`.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use paraclete_node_api::{LedUpdate, RgbColor};

use crate::protocol::{LedMsg, ServerMsg};

/// Fixed client capacity. The 5th concurrent connection is refused with
/// `bye {"reason":"full"}`.
pub const MAX_CLIENTS: usize = 4;

/// Shadow table size: control ids 0–97 (80 pads, gap 80–89, encoders 90–97).
pub const SHADOW_SLOTS: usize = 98;

// ── Client table ──────────────────────────────────────────────────────────────

/// One connected client's outbound half.
pub struct ClientSlot {
    /// Serialized frames; the client's I/O thread owns the receiver.
    pub sender: mpsc::Sender<String>,
    /// Set at allocation; cleared by kerygma after the full-surface LED replay.
    pub needs_replay: bool,
    /// Set at allocation; cleared by `AntiphonHandle::pump()` after the full
    /// state + context replay. Without it a client connecting after startup
    /// never sees values that last changed before it connected — the mirror
    /// only sends diffs (found 2026-07-10: fresh clients showed no BPM/mode).
    pub needs_state_replay: bool,
}

/// Fixed-slot registry of connected clients. Shared between the accept/IO
/// threads (allocate/free) and the main thread (kerygma broadcast) behind a
/// `Mutex` — never touched by the audio thread.
#[derive(Default)]
pub struct ClientTable {
    slots: [Option<ClientSlot>; MAX_CLIENTS],
}

impl ClientTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Claim a free slot for a new client. Returns the slot index, or `None`
    /// when all `MAX_CLIENTS` slots are taken.
    pub fn allocate(&mut self, sender: mpsc::Sender<String>) -> Option<usize> {
        let idx = self.slots.iter().position(|s| s.is_none())?;
        self.slots[idx] =
            Some(ClientSlot { sender, needs_replay: true, needs_state_replay: true });
        Some(idx)
    }

    /// Release a slot on disconnect, making it reusable.
    pub fn free(&mut self, idx: usize) {
        if idx < MAX_CLIENTS {
            self.slots[idx] = None;
        }
    }

    pub fn active_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Send an already-serialized frame to every connected client.
    /// Send errors are ignored — a dead client's I/O thread frees its own slot.
    pub fn send_to_all(&self, frame: &str) {
        for slot in self.slots.iter().flatten() {
            let _ = slot.sender.send(frame.to_owned());
        }
    }

    fn for_each_replay_pending(&mut self, mut f: impl FnMut(&ClientSlot)) {
        for slot in self.slots.iter_mut().flatten() {
            if slot.needs_replay {
                f(slot);
                slot.needs_replay = false;
            }
        }
    }

    /// True if any connected client still awaits its state/context replay.
    /// Cheap gate so `pump()` pays nothing on the common no-new-client tick.
    pub fn any_state_replay_pending(&self) -> bool {
        self.slots.iter().flatten().any(|s| s.needs_state_replay)
    }

    /// Visit every client awaiting a state/context replay, clearing the flag.
    pub fn for_each_state_replay_pending(&mut self, mut f: impl FnMut(&ClientSlot)) {
        for slot in self.slots.iter_mut().flatten() {
            if slot.needs_state_replay {
                f(slot);
                slot.needs_state_replay = false;
            }
        }
    }
}

// ── Kerygma ───────────────────────────────────────────────────────────────────

/// LED broadcast with a main-thread shadow copy of the last known colour per
/// control, replayed to newly connected clients so a fresh surface shows the
/// current state rather than darkness.
pub struct Kerygma {
    clients: Arc<Mutex<ClientTable>>,
    shadow: [RgbColor; SHADOW_SLOTS],
}

impl Kerygma {
    pub fn new(clients: Arc<Mutex<ClientTable>>) -> Self {
        Self { clients, shadow: [RgbColor::OFF; SHADOW_SLOTS] }
    }

    /// Update the shadow and fan a `led` batch out to all connected clients.
    pub fn broadcast_led_updates(&mut self, updates: &[LedUpdate]) {
        if updates.is_empty() {
            return;
        }
        let mut msgs = Vec::with_capacity(updates.len());
        for u in updates {
            if (u.control_id as usize) < SHADOW_SLOTS {
                self.shadow[u.control_id as usize] = u.color;
                msgs.push(LedMsg { id: u.control_id, rgb: [u.color.r, u.color.g, u.color.b] });
            } else {
                eprintln!("[antiphon] led update for unknown control id {}", u.control_id);
            }
        }
        if msgs.is_empty() {
            return;
        }
        let Ok(frame) = serde_json::to_string(&ServerMsg::Led { updates: msgs }) else {
            return;
        };
        let Ok(table) = self.clients.lock() else { return };
        table.send_to_all(&frame);
    }

    /// Send the full-surface shadow state to any client flagged `needs_replay`
    /// (i.e. connected since the last tick). Called every main-loop tick,
    /// before the regular batch broadcast.
    pub fn service_replays(&mut self) {
        let Ok(mut table) = self.clients.lock() else { return };
        // Built lazily: zero cost on the (overwhelmingly common) no-new-client tick.
        let mut frame: Option<String> = None;
        let shadow = &self.shadow;
        table.for_each_replay_pending(|slot| {
            let frame = frame.get_or_insert_with(|| {
                let updates = shadow
                    .iter()
                    .enumerate()
                    .filter(|(id, _)| !(80..90).contains(id)) // gap between pads and encoders
                    .map(|(id, c)| LedMsg { id: id as u32, rgb: [c.r, c.g, c.b] })
                    .collect();
                serde_json::to_string(&ServerMsg::Led { updates }).unwrap_or_default()
            });
            if !frame.is_empty() {
                let _ = slot.sender.send(frame.clone());
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Arc<Mutex<ClientTable>> {
        Arc::new(Mutex::new(ClientTable::new()))
    }

    #[test]
    fn fifth_client_rejected_full() {
        let t = table();
        let mut guard = t.lock().unwrap();
        let mut rxs = Vec::new();
        for i in 0..MAX_CLIENTS {
            let (tx, rx) = mpsc::channel();
            rxs.push(rx);
            assert_eq!(guard.allocate(tx), Some(i));
        }
        let (tx5, _rx5) = mpsc::channel();
        assert_eq!(guard.allocate(tx5), None, "5th client must be refused");
        assert_eq!(guard.active_count(), MAX_CLIENTS);
    }

    #[test]
    fn slot_freed_on_disconnect_and_reusable() {
        let t = table();
        let mut guard = t.lock().unwrap();
        let (tx_a, _rx_a) = mpsc::channel();
        let (tx_b, _rx_b) = mpsc::channel();
        let (tx_c, _rx_c) = mpsc::channel();
        let a = guard.allocate(tx_a).unwrap();
        let _b = guard.allocate(tx_b).unwrap();
        guard.free(a);
        assert_eq!(guard.active_count(), 1);
        assert_eq!(guard.allocate(tx_c), Some(a), "freed slot must be reused");
    }

    #[test]
    fn kerygma_shadow_replays_full_surface_to_new_client() {
        let t = table();
        let mut k = Kerygma::new(Arc::clone(&t));

        // Set 3 LEDs before anyone is connected.
        k.broadcast_led_updates(&[
            LedUpdate { control_id: 0, color: RgbColor::RED },
            LedUpdate { control_id: 13, color: RgbColor { r: 0, g: 64, b: 255 } },
            LedUpdate { control_id: 64, color: RgbColor::GREEN },
        ]);

        // A client connects (allocation flags it for replay).
        let (tx, rx) = mpsc::channel();
        t.lock().unwrap().allocate(tx).unwrap();
        k.service_replays();

        let frame = rx.try_recv().expect("replay batch must arrive");
        let msg: ServerMsg = serde_json::from_str(&frame).unwrap();
        let ServerMsg::Led { updates } = msg else {
            panic!("expected led batch, got {frame}")
        };
        // Full surface minus the 80–89 id gap between pads and encoders.
        assert_eq!(updates.len(), SHADOW_SLOTS - 10, "replay is full-surface");
        assert_eq!(updates[0].rgb, [255, 0, 0]);
        assert_eq!(updates[13].rgb, [0, 64, 255]);
        assert_eq!(updates[64].rgb, [0, 255, 0]);
        assert_eq!(updates[1].rgb, [0, 0, 0], "unset controls replay as off");

        // Replay happens exactly once.
        k.service_replays();
        assert!(rx.try_recv().is_err(), "no second replay");
    }

    #[test]
    fn broadcast_reaches_all_connected_clients_and_updates_shadow() {
        let t = table();
        let mut k = Kerygma::new(Arc::clone(&t));
        let (tx_a, rx_a) = mpsc::channel();
        let (tx_b, rx_b) = mpsc::channel();
        {
            let mut guard = t.lock().unwrap();
            guard.allocate(tx_a).unwrap();
            guard.allocate(tx_b).unwrap();
        }
        k.service_replays(); // consume the connect replays
        let _ = rx_a.try_recv();
        let _ = rx_b.try_recv();

        k.broadcast_led_updates(&[LedUpdate { control_id: 7, color: RgbColor::WHITE }]);
        for rx in [&rx_a, &rx_b] {
            let frame = rx.try_recv().expect("both clients receive the batch");
            let ServerMsg::Led { updates } = serde_json::from_str(&frame).unwrap() else {
                panic!("expected led")
            };
            assert_eq!(updates, vec![LedMsg { id: 7, rgb: [255, 255, 255] }]);
        }
    }
}
