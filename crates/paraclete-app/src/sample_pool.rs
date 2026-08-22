// SPDX-License-Identifier: GPL-3.0-or-later
//! Sample pool: project-level sample assets that samplers reference by
//! stepped index (AN0, ADR-040 decision 3).
//!
//! A pool entry is app-owned — decoding happens on the main thread, never
//! in `process()`. The decoded audio reaches nodes over the asset-delivery
//! plane (`SwapSample`, added in AN0 C1), not the command plane.

use std::collections::HashMap;
use std::sync::Arc;

use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use serde::{Deserialize, Serialize};

/// Nominal pool capacity (AN0 §0 D2). The pool is `Vec`-backed, not array-backed —
/// a slot beyond this length grows the vec with `None` padding rather than
/// being rejected, so this is a UI/param-range convenience, not a hard cap.
pub const NOMINAL_POOL_SIZE: usize = 128;

/// Where a pool entry's audio data comes from.
///
/// `Ram` is declared now (AN0) even though nothing constructs it yet —
/// AN1's recorder capture fills in the real payload later without a
/// breaking enum change to a shape already saved in projects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SampleSource {
    File(String),
    Ram { frames: usize },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PoolEntry {
    pub name: String,
    pub source: SampleSource,
    pub frames: usize,
    pub rate: f32,
    pub bars: Option<f32>,
    pub gain: f64,
    /// (start_frame, end_frame) pairs. AN0 populates the trivial single
    /// full-sample slice; AN2 gives this a real slicing UI.
    pub slice_table: Vec<(usize, usize)>,
}

/// Project-level sample pool. Slot `None` = empty. Mirrors `KitStore`'s
/// shape (`crates/paraclete-app/src/kit.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplePool {
    pub slots: Vec<Option<PoolEntry>>,
    /// AN1 D14: decoded PCM for `Ram`-sourced slots only — `File` slots
    /// never need it, they already re-decode from `path` on demand. Not
    /// persisted (a project file only ever holds metadata; a `Ram` slot
    /// is frozen to `File` before it's ever written out —
    /// `freeze_ram_captures_to_files`). Populated by `record_capture`;
    /// an entry is dropped once its slot is frozen, since nothing reads
    /// it again after that point.
    #[serde(skip)]
    pub ram_cache: HashMap<usize, Arc<Vec<f32>>>,
}

impl Default for SamplePool {
    fn default() -> Self {
        Self { slots: vec![None; NOMINAL_POOL_SIZE], ram_cache: HashMap::new() }
    }
}

impl SamplePool {
    pub fn get(&self, slot: usize) -> Option<&PoolEntry> {
        self.slots.get(slot).and_then(|o| o.as_ref())
    }

    /// Decode `path` at `target_rate` and store it at `slot`, growing the
    /// pool with `None` padding if `slot` is beyond the current length.
    /// Returns the new entry and the decoded samples for the caller to
    /// distribute to live nodes via the asset-delivery plane.
    pub fn load_file(
        &mut self,
        slot: usize,
        path: &str,
        target_rate: f32,
    ) -> Result<(&PoolEntry, Vec<f32>), String> {
        let (samples, frames, rate) = decode_file(path, target_rate)?;
        let entry = PoolEntry {
            name: std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path)
                .to_string(),
            source: SampleSource::File(path.to_string()),
            frames,
            rate,
            bars: None,
            gain: 1.0,
            slice_table: vec![(0, frames)],
        };
        if slot >= self.slots.len() {
            self.slots.resize(slot + 1, None);
        }
        self.slots[slot] = Some(entry);
        Ok((self.slots[slot].as_ref().expect("just inserted"), samples))
    }

    /// AN1 D13/D14 (§2.3): record a captured buffer into the first free
    /// slot (a capture assigns itself a slot — unlike `load_file`, there
    /// is no operator-chosen target). Stores a `Ram`-sourced entry and
    /// caches the PCM in `ram_cache` (needed later for redistribution and
    /// for `freeze_ram_captures_to_files`). Returns the assigned slot and
    /// an owned copy of the samples for the caller to distribute via
    /// `SwapSample`, mirroring `load_file`'s calling convention.
    pub fn record_capture(
        &mut self,
        samples: &[f32],
        rate: f32,
        bars: Option<f32>,
    ) -> (usize, Vec<f32>) {
        let slot = self.slots.iter().position(|s| s.is_none()).unwrap_or(self.slots.len());
        let frames = samples.len();
        let entry = PoolEntry {
            name: format!("capture_{slot}"),
            source: SampleSource::Ram { frames },
            frames,
            rate,
            bars,
            gain: 1.0,
            slice_table: vec![(0, frames)],
        };
        if slot >= self.slots.len() {
            self.slots.resize(slot + 1, None);
        }
        self.slots[slot] = Some(entry);
        let owned = samples.to_vec();
        self.ram_cache.insert(slot, Arc::new(owned.clone()));
        (slot, owned)
    }

    /// AN1 D13 (OQ-A2 resolved): convert every `Ram`-sourced slot to
    /// `File`, writing its cached PCM to `{project_dir}/captures/
    /// slot_{N}.wav`. Call this once, right before `save_project_v3` —
    /// after it returns, every slot is either `None` or `File`, so the
    /// save path itself needs no `Ram`-vs-`File` awareness. Returns one
    /// warning string per slot that failed to freeze (dropped rather than
    /// left as a dangling `Ram` reference with no cached PCM to write —
    /// D14 says nothing reads `ram_cache` again after this runs, so a
    /// slot this can't freeze has no path back to real audio either way).
    pub fn freeze_ram_captures_to_files(&mut self, project_dir: &std::path::Path) -> Vec<String> {
        let mut warnings = Vec::new();
        let captures_dir = project_dir.join("captures");
        for (slot_idx, entry_opt) in self.slots.iter_mut().enumerate() {
            let Some(entry) = entry_opt else { continue };
            if !matches!(entry.source, SampleSource::Ram { .. }) {
                continue;
            }
            let Some(pcm) = self.ram_cache.get(&slot_idx) else {
                warnings.push(format!(
                    "slot {slot_idx}: Ram entry with no cached PCM — dropping (nothing to freeze)"
                ));
                *entry_opt = None;
                continue;
            };
            if let Err(e) = std::fs::create_dir_all(&captures_dir) {
                warnings.push(format!("slot {slot_idx}: could not create captures dir: {e}"));
                continue;
            }
            let file_path = captures_dir.join(format!("slot_{slot_idx}.wav"));
            match write_wav(&file_path, pcm, entry.rate) {
                Ok(()) => entry.source = SampleSource::File(file_path.to_string_lossy().into_owned()),
                Err(e) => warnings.push(format!("slot {slot_idx}: failed to freeze capture: {e}")),
            }
        }
        self.ram_cache.clear();
        warnings
    }
}

/// Write mono `samples` as 16-bit PCM WAV at `rate` — the write-side
/// counterpart to `decode_file` (which reads WAV via symphonia; any
/// standard 16-bit PCM WAV is a format symphonia already reads, so no new
/// read-path work is needed for a frozen capture to load back in). Plain
/// byte-level RIFF/WAVE construction, no new dependency — mirrors the
/// exact layout this module's own `decode_file` tests already write by
/// hand for their fixtures.
fn write_wav(path: &std::path::Path, samples: &[f32], rate: f32) -> Result<(), String> {
    let rate = rate.round().max(1.0) as u32;
    let data_bytes = (samples.len() * 2) as u32;
    let mut buf = Vec::with_capacity(44 + data_bytes as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&rate.to_le_bytes());
    buf.extend_from_slice(&(rate * 2).to_le_bytes()); // byte rate
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_bytes.to_le_bytes());
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let quantized = (clamped * i16::MAX as f32) as i16;
        buf.extend_from_slice(&quantized.to_le_bytes());
    }
    std::fs::write(path, buf).map_err(|e| format!("write: {e}"))
}

// ── Audio file decoding via symphonia (hoisted from paraclete-nodes'
// Sampler::load_wav — AN0 moves decode-at-load-time to the pool; a
// duplicate copy stays in paraclete-nodes until AN0 C2 rewires `Sampler`
// to stop loading files itself, since L3 (paraclete-nodes) may not depend
// on the App layer.) ─────────────────────────────────────────────────────

/// Decode an audio file (WAV/FLAC/AIFF/OGG/MP3 via symphonia) to mono
/// samples at `target_rate`. Returns `(samples, frame_count, rate)`.
/// Main-thread only — never called from `process()`.
pub fn decode_file(path: &str, target_rate: f32) -> Result<(Vec<f32>, usize, f32), String> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let hint = Hint::new(); // probe by magic bytes, not extension
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &Default::default(), &Default::default())
        .map_err(|e| format!("probe: {e}"))?;
    let mut reader = probed.format;
    let track = reader
        .default_track()
        .ok_or_else(|| "no audio track".to_string())?;
    let codec_params = track.codec_params.clone();
    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &Default::default())
        .map_err(|e| format!("codec: {e}"))?;

    let native_rate = codec_params
        .sample_rate
        .ok_or_else(|| "audio file has no sample rate in codec metadata".to_string())?
        as f32;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut channels = 1usize;

    loop {
        match reader.next_packet() {
            Ok(packet) => {
                let decoded = decoder.decode(&packet).map_err(|e| format!("decode: {e}"))?;
                let spec = *decoded.spec();
                channels = spec.channels.count();
                let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
                buf.copy_interleaved_ref(decoded);
                interleaved.extend_from_slice(buf.samples());
            }
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(e) => return Err(format!("read: {e}")),
        }
    }

    // Deinterleave to mono (AN0 §0 D5 — inherited limitation, not new).
    let mono: Vec<f32> = if channels == 1 {
        interleaved
    } else {
        interleaved
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    };

    let samples = if (native_rate - target_rate).abs() < 1.0 {
        mono
    } else {
        resample_sinc(&mono, native_rate, target_rate)?
    };
    let frames = samples.len();
    Ok((samples, frames, target_rate))
}

fn resample_sinc(samples: &[f32], from_rate: f32, to_rate: f32) -> Result<Vec<f32>, String> {
    if samples.is_empty() {
        return Ok(vec![]);
    }

    let ratio = to_rate as f64 / from_rate as f64;
    let chunk = 512usize;
    let params = SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 64,
        window: WindowFunction::BlackmanHarris2,
    };
    let mut resampler = SincFixedIn::<f32>::new(ratio, 2.0, params, chunk, 1)
        .map_err(|e| format!("rubato init: {e}"))?;

    let expected = (samples.len() as f64 * ratio) as usize;
    let mut output = Vec::with_capacity(expected + chunk);
    let mut pos = 0;

    loop {
        let end = (pos + chunk).min(samples.len());
        let mut buf_in = samples[pos..end].to_vec();
        buf_in.resize(chunk, 0.0);

        match resampler.process(&[buf_in], None) {
            Ok(out_channels) => {
                if let Some(ch) = out_channels.into_iter().next() {
                    output.extend_from_slice(&ch);
                }
            }
            Err(e) => return Err(format!("rubato process: {e}")),
        }

        if end >= samples.len() {
            break;
        }
        pos += chunk;
    }

    output.truncate(expected);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_minimal_wav(path: &std::path::Path, sample_rate: u32, frames: usize) {
        let data_bytes = (frames * 2) as u32;
        let mut buf = Vec::with_capacity(44 + data_bytes as usize);
        buf.extend_from_slice(b"RIFF");
        buf.extend_from_slice(&(36 + data_bytes).to_le_bytes());
        buf.extend_from_slice(b"WAVE");
        buf.extend_from_slice(b"fmt ");
        buf.extend_from_slice(&16u32.to_le_bytes());
        buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
        buf.extend_from_slice(&1u16.to_le_bytes()); // mono
        buf.extend_from_slice(&sample_rate.to_le_bytes());
        buf.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        buf.extend_from_slice(&2u16.to_le_bytes()); // block align
        buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        buf.extend_from_slice(b"data");
        buf.extend_from_slice(&data_bytes.to_le_bytes());
        for i in 0..frames {
            let t = i as f32 / sample_rate as f32;
            let sample = (t * 440.0 * std::f32::consts::TAU).sin() * 8000.0;
            buf.extend_from_slice(&(sample as i16).to_le_bytes());
        }
        std::fs::write(path, buf).expect("write wav");
    }

    #[test]
    fn decode_file_reads_a_real_wav() {
        let tmp = std::env::temp_dir().join("an0_sample_pool_decode_test.wav");
        write_minimal_wav(&tmp, 44100, 512);

        let (samples, frames, rate) = decode_file(tmp.to_str().unwrap(), 44100.0).expect("decode");
        assert_eq!(frames, 512);
        assert_eq!(samples.len(), 512);
        assert_eq!(rate, 44100.0);
        assert!(samples.iter().any(|&x| x.abs() > 0.0));

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn decode_file_resamples_to_target_rate() {
        let tmp = std::env::temp_dir().join("an0_sample_pool_resample_test.wav");
        write_minimal_wav(&tmp, 44100, 1024);

        let (samples, frames, rate) = decode_file(tmp.to_str().unwrap(), 48000.0).expect("decode");
        assert_eq!(rate, 48000.0);
        // 1024 frames @ 44100 -> ~1115 frames @ 48000
        assert!(frames > 1024, "resampled frame count should grow with rate");
        assert_eq!(samples.len(), frames);

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn load_file_stores_entry_and_grows_pool() {
        let tmp = std::env::temp_dir().join("an0_sample_pool_load_file_test.wav");
        write_minimal_wav(&tmp, 44100, 256);

        let mut pool = SamplePool { slots: vec![None; 4], ..Default::default() };
        let (entry, samples) = pool
            .load_file(2, tmp.to_str().unwrap(), 44100.0)
            .expect("load_file");
        assert_eq!(entry.frames, 256);
        assert!(matches!(entry.source, SampleSource::File(_)));
        assert_eq!(samples.len(), 256);
        assert_eq!(pool.slots.len(), 4, "no growth needed within bounds");

        // Out-of-range slot grows the pool.
        pool.load_file(10, tmp.to_str().unwrap(), 44100.0).expect("load_file");
        assert_eq!(pool.slots.len(), 11);
        assert!(pool.get(10).is_some());
        assert!(pool.get(9).is_none());

        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn default_pool_has_nominal_size_all_empty() {
        let pool = SamplePool::default();
        assert_eq!(pool.slots.len(), NOMINAL_POOL_SIZE);
        assert!(pool.slots.iter().all(|s| s.is_none()));
    }

    /// AN0 §0 D1: PoolEntry list must round-trip through RON exactly like
    /// `Kit`'s existing `kits_section_ron_round_trip` test (kit.rs).
    #[test]
    fn pool_entries_ron_round_trip() {
        let mut slots: Vec<Option<PoolEntry>> = vec![None; 4];
        slots[1] = Some(PoolEntry {
            name: "kick".into(),
            source: SampleSource::File("samples/kick.wav".into()),
            frames: 4096,
            rate: 48000.0,
            bars: Some(2.0),
            gain: 0.8,
            slice_table: vec![(0, 4096)],
        });

        let ron_str = ron::ser::to_string(&slots).expect("serialize");
        let back: Vec<Option<PoolEntry>> = ron::de::from_str(&ron_str).expect("deserialize");

        assert_eq!(back.len(), 4);
        let Some(e) = &back[1] else { panic!("slot 1 must survive") };
        assert_eq!(e.name, "kick");
        assert!(matches!(&e.source, SampleSource::File(p) if p == "samples/kick.wav"));
        assert_eq!(e.frames, 4096);
        assert_eq!(e.rate, 48000.0);
        assert_eq!(e.bars, Some(2.0));
        assert_eq!(e.gain, 0.8);
        assert_eq!(e.slice_table, vec![(0, 4096)]);
        assert!(back[0].is_none() && back[2].is_none());
    }

    #[test]
    fn record_capture_assigns_the_first_free_slot_and_caches_pcm() {
        let mut pool = SamplePool::default();
        let samples = vec![0.1, 0.2, 0.3];
        let (slot, returned) = pool.record_capture(&samples, 44100.0, Some(2.0));
        assert_eq!(slot, 0, "first free slot in a fresh pool is 0");
        assert_eq!(returned, samples);
        let entry = pool.get(0).expect("slot must be populated");
        assert!(matches!(entry.source, SampleSource::Ram { frames: 3 }));
        assert_eq!(entry.frames, 3);
        assert_eq!(entry.bars, Some(2.0));
        assert_eq!(pool.ram_cache.get(&0).map(|a| a.as_ref().clone()), Some(samples));
    }

    #[test]
    fn record_capture_skips_occupied_slots() {
        let mut pool = SamplePool::default();
        pool.slots[0] = Some(PoolEntry {
            name: "occupied".into(),
            source: SampleSource::File("x.wav".into()),
            frames: 1,
            rate: 44100.0,
            bars: None,
            gain: 1.0,
            slice_table: vec![(0, 1)],
        });
        let (slot, _) = pool.record_capture(&[1.0], 44100.0, None);
        assert_eq!(slot, 1, "slot 0 is occupied, must not overwrite it");
    }

    /// AN1 D13/D14: the save/reload round trip a `Ram` slot must survive —
    /// freeze writes a real WAV file, converts the slot to `File`, and a
    /// fresh `decode_file` on that path reads back the same audio (within
    /// 16-bit PCM quantization).
    #[test]
    fn freeze_converts_ram_to_file_and_the_frozen_file_decodes_back_to_the_same_audio() {
        let tmp = std::env::temp_dir().join("an1_freeze_test_project");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut pool = SamplePool::default();
        let samples: Vec<f32> = (0..256)
            .map(|i| (i as f32 / 256.0 * std::f32::consts::TAU).sin() * 0.5)
            .collect();
        let (slot, _) = pool.record_capture(&samples, 44100.0, None);

        let warnings = pool.freeze_ram_captures_to_files(&tmp);
        assert!(warnings.is_empty(), "a well-formed capture must freeze cleanly: {warnings:?}");

        let entry = pool.get(slot).expect("slot must survive freeze");
        let SampleSource::File(path) = &entry.source else {
            panic!("frozen slot must be SampleSource::File, not Ram")
        };
        assert!(std::path::Path::new(path).exists(), "the WAV file must actually exist on disk");

        let (decoded, frames, rate) = decode_file(path, 44100.0).expect("frozen WAV must decode");
        assert_eq!(frames, 256);
        assert_eq!(rate, 44100.0);
        for (original, back) in samples.iter().zip(decoded.iter()) {
            assert!(
                (original - back).abs() < 0.001,
                "round-tripped sample drifted beyond 16-bit quantization: {original} vs {back}"
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn freeze_clears_ram_cache_since_nothing_reads_it_again() {
        let tmp = std::env::temp_dir().join("an1_freeze_cache_evict_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        let mut pool = SamplePool::default();
        pool.record_capture(&[0.1, 0.2, 0.3], 44100.0, None);
        assert_eq!(pool.ram_cache.len(), 1);

        pool.freeze_ram_captures_to_files(&tmp);
        assert!(pool.ram_cache.is_empty(), "ram_cache must be evicted after every slot is frozen");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn freeze_is_a_no_op_for_file_sourced_slots() {
        let tmp = std::env::temp_dir().join("an1_freeze_noop_test");
        let mut pool = SamplePool::default();
        pool.slots[0] = Some(PoolEntry {
            name: "already_file".into(),
            source: SampleSource::File("some/existing/path.wav".into()),
            frames: 10,
            rate: 44100.0,
            bars: None,
            gain: 1.0,
            slice_table: vec![(0, 10)],
        });
        let warnings = pool.freeze_ram_captures_to_files(&tmp);
        assert!(warnings.is_empty());
        let entry = pool.get(0).unwrap();
        assert!(
            matches!(&entry.source, SampleSource::File(p) if p == "some/existing/path.wav"),
            "an already-File slot must be untouched, not rewritten"
        );
    }

    /// A `Ram` entry with no corresponding `ram_cache` entry (shouldn't
    /// happen via the normal `record_capture` path, but is a real
    /// deserialize-time possibility if a future format ever persisted a
    /// bare `Ram` marker) must be dropped cleanly with a warning, not
    /// left as a dangling reference to audio that no longer exists.
    #[test]
    fn freeze_drops_a_ram_slot_with_no_cached_pcm_and_warns() {
        let tmp = std::env::temp_dir().join("an1_freeze_no_cache_test");
        let mut pool = SamplePool::default();
        pool.slots[0] = Some(PoolEntry {
            name: "orphaned".into(),
            source: SampleSource::Ram { frames: 5 },
            frames: 5,
            rate: 44100.0,
            bars: None,
            gain: 1.0,
            slice_table: vec![(0, 5)],
        });
        // No corresponding ram_cache entry — record_capture normally
        // populates both together; this simulates the entry existing
        // without it.
        let warnings = pool.freeze_ram_captures_to_files(&tmp);
        assert_eq!(warnings.len(), 1);
        assert!(pool.get(0).is_none(), "an unfreezeable Ram slot must be dropped, not left dangling");
    }
}
