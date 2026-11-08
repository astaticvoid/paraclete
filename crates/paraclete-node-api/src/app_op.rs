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
    /// AN0 C5: decode `path` into the sample pool at `slot`, then deliver
    /// the result to every live Sampler node's asset cache. The one
    /// runtime pool-population path AN0 ships (no capture yet — this is
    /// how a session populates a slot without editing the project file).
    PoolLoadFile { slot: u32, path: String },
    /// AN1 C4: arm `recorder_id` for a threshold-triggered capture at
    /// `db`. Sent as `CMD_RECORDER_ARM_THRESHOLD`.
    RecorderArmThreshold { recorder_id: u32, db: f64 },
    /// AN1 C4: disarm `recorder_id`, clearing any pending threshold-arm
    /// state. Sent as `CMD_RECORDER_DISARM`.
    RecorderDisarm { recorder_id: u32 },
    /// AN1 C4: capture the last `bars` bars from `recorder_id`'s
    /// always-on ring into a new pool slot (design.md §4's Retro row, or
    /// the app's own reaction to a threshold-arm firing, §2.4). `bars` is
    /// converted to frames against the current tempo at the moment this
    /// op is executed, not when it was queued.
    RecorderCapture { recorder_id: u32, bars: f32 },
    /// AN1 C4, R2's frozen requirement: the `FUNC+9` one-gesture
    /// transition trick — capture the last 2 bars of `recorder_id` into a
    /// new pool slot, apply a 16-slot even slice grid, and deliver it to
    /// `sampler_id` via `SwapSample`, all as one composite op.
    CaptureTransition { recorder_id: u32, sampler_id: u32 },
    /// Save the current project state (nodes, kits, sample pool + frozen RAM
    /// captures) to `path` (or the session's default project path if `None`).
    SaveProject { path: Option<String> },
    /// Load a saved project file at runtime via pause-restore-rebuild-resume.
    LoadProject { path: String },
}
