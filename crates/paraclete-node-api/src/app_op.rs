//! App-level operation vocabulary shared across surfaces.
//!
//! `AppOp` commands originate from Theotokos (key chords, KIT screen)
//! and Antiphon (Theoria web UI) and are drained by the app main loop
//! each tick.  The types live here in `paraclete-node-api` so both
//! surface crates can produce them without depending on `paraclete-app`.

/// Identifies a kit slot in the 64-kit store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KitId(pub u8);

impl KitId {
    /// Maximum valid kit id (0-based).  Values ≥ 64 are rejected.
    pub const MAX: u8 = 63;
}

/// App-level operation produced by a surface and consumed by the
/// main loop's `PerformState`.
#[derive(Debug, Clone)]
pub enum AppOp {
    /// FUNC + YES: snapshot current parameter+pattern state.
    TempSave,
    /// FUNC + NO: reload the snapshot.
    TempReload,
    /// Load (apply) a saved kit.
    KitLoad(KitId),
    /// Capture current state into a named kit slot.
    KitSaveAs(String),
    /// Capture current state back into the currently-bound kit.
    KitCommit,
    /// Re-apply the currently-bound kit.
    KitReload,
    /// Bind (or unbind) a kit to a pattern slot.
    BindKit { slot: usize, kit: Option<KitId> },
    /// Toggle perform mode (kit-apply on pattern change).
    SetPerformMode(bool),
}
