//! Correlated diagnostic logger for the vehicle scanner + HRTF chain, from
//! scanner_hrtf_diag.py.
//!
//! The scanner beep pitches up as a target moves toward dead ahead while the HRTF should
//! place the image dead centre; users reported the two not coinciding. This records, per
//! scanner beep frame, the bearing, the pitch it was played at, the HRTF azimuth, the
//! IR's intrinsic interaural level difference and the convolved output's, so the cause
//! (tail smear, IR asymmetry, a bearing zero that is not "facing") can be told apart.
//!
//! Enabled by the environment variable BEAM_SCANNER_DIAG (any non-empty value). Output:
//! `%LOCALAPPDATA%\beamtel\scanner_hrtf_diag.csv`, fresh each session. Cheap on the audio
//! thread: `record_beep` computes two RMS values and queues one row; a writer thread
//! drains the queue to disk every 0.25 s, so no file I/O happens in the callback.

use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use beam_base::pyfmt::py_f;
use parking_lot::{Condvar, Mutex};

use crate::dsp;

pub const CSV_HEADER: &str = "t_ms,dt_ms,bearing_deg,dist_m,aligned,pitch_norm,pitch_mult,\
hrtf_used,hrtf_az_deg,ir_ild_db,out_ild_db,rms_l,rms_r\n";

/// The CSV's file name inside the config directory.
pub const FILE_NAME: &str = "scanner_hrtf_diag.csv";

/// `DEFAULT_PATH`: `%LOCALAPPDATA%\beamtel\scanner_hrtf_diag.csv` (home when LOCALAPPDATA
/// is unset). Module level in Python so the View Logs menu can offer the file.
pub fn default_path() -> PathBuf {
    beam_base::paths::config_dir().join(FILE_NAME)
}

/// Root-mean-square of a float64 buffer; 0.0 for empty or `None`.
pub fn rms(arr: Option<&[f64]>) -> f64 {
    match arr {
        Some(a) if !a.is_empty() => dsp::rms(a),
        _ => 0.0,
    }
}

/// [`rms`] of a float32 buffer, squared in float64 as Python's `np.square(arr,
/// dtype=np.float64)` does.
pub fn rms_f32(arr: Option<&[f32]>) -> f64 {
    match arr {
        Some(a) if !a.is_empty() => {
            let w: Vec<f64> = a.iter().map(|&v| v as f64).collect();
            dsp::rms(&w)
        }
        _ => 0.0,
    }
}

/// Interaural level difference in dB. Positive means LEFT louder. 0.0 when both ears are
/// effectively silent, so an idle frame does not read as a huge imbalance.
pub fn ild_db(rms_l: f64, rms_r: f64) -> f64 {
    let eps = 1e-9;
    if rms_l < eps && rms_r < eps {
        return 0.0;
    }
    20.0 * ((rms_l + eps) / (rms_r + eps)).log10()
}

/// One queued row, in CSV column order.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeepRow {
    pub t_ms: f64,
    pub dt_ms: f64,
    pub bearing_deg: f64,
    pub dist_m: f64,
    pub aligned: bool,
    pub pitch_norm: f64,
    pub pitch_mult: f64,
    pub hrtf_used: bool,
    pub hrtf_az_deg: f64,
    pub ir_ild_db: f64,
    pub out_ild_db: f64,
    pub rms_l: f64,
    pub rms_r: f64,
}

impl BeepRow {
    /// `"%.3f,%.3f,%.4f,%.3f,%d,%.4f,%.4f,%d,%.3f,%.3f,%.3f,%.6f,%.6f\n" % row`.
    pub fn csv_line(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
            py_f(self.t_ms, 3),
            py_f(self.dt_ms, 3),
            py_f(self.bearing_deg, 4),
            py_f(self.dist_m, 3),
            u8::from(self.aligned),
            py_f(self.pitch_norm, 4),
            py_f(self.pitch_mult, 4),
            u8::from(self.hrtf_used),
            py_f(self.hrtf_az_deg, 3),
            py_f(self.ir_ild_db, 3),
            py_f(self.out_ild_db, 3),
            py_f(self.rms_l, 6),
            py_f(self.rms_r, 6),
        )
    }
}

/// `time.perf_counter() * 1000.0`: milliseconds on a monotonic clock with an arbitrary
/// origin.
fn perf_counter_ms() -> f64 {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

struct Shared {
    path: PathBuf,
    enabled: AtomicBool,
    buf: Mutex<VecDeque<BeepRow>>,
    stop: Mutex<bool>,
    wake: Condvar,
}

impl Shared {
    fn flush(&self) {
        let rows: Vec<BeepRow> = {
            let mut b = self.buf.lock();
            if b.is_empty() {
                return;
            }
            b.drain(..).collect()
        };
        let result = OpenOptions::new()
            .append(true)
            .create(true)
            .open(&self.path)
            .and_then(|mut f| {
                for r in &rows {
                    f.write_all(r.csv_line().as_bytes())?;
                }
                Ok(())
            });
        if let Err(e) = result {
            // Don't spam; disable on persistent failure.
            log::warn!("Scanner HRTF diagnostic write failed: {e}");
            self.enabled.store(false, Ordering::Relaxed);
        }
    }

    fn drain_loop(&self) {
        loop {
            self.flush();
            let mut stopped = self.stop.lock();
            if *stopped {
                break;
            }
            self.wake.wait_for(&mut stopped, Duration::from_millis(250));
            if *stopped {
                break;
            }
        }
        self.flush();
    }
}

/// Lightweight, audio-thread-safe recorder for scanner/HRTF correlation.
pub struct ScannerHrtfDiagnostic {
    shared: Arc<Shared>,
    last_t: Mutex<Option<f64>>,
    writer: Option<std::thread::JoinHandle<()>>,
}

impl ScannerHrtfDiagnostic {
    /// `ScannerHRTFDiagnostic(logger, path)`: enabled when BEAM_SCANNER_DIAG is set to a
    /// non-empty value. `None` takes [`default_path`].
    pub fn new(path: Option<PathBuf>) -> Self {
        let enabled = std::env::var_os("BEAM_SCANNER_DIAG").is_some_and(|v| !v.is_empty());
        Self::with_enabled(path, enabled)
    }

    /// [`ScannerHrtfDiagnostic::new`] with the enable decision passed in, for tests.
    pub fn with_enabled(path: Option<PathBuf>, enabled: bool) -> Self {
        let path = path.unwrap_or_else(default_path);
        let shared = Arc::new(Shared {
            path: path.clone(),
            enabled: AtomicBool::new(enabled),
            buf: Mutex::new(VecDeque::new()),
            stop: Mutex::new(false),
            wake: Condvar::new(),
        });
        let mut me = ScannerHrtfDiagnostic {
            shared: shared.clone(),
            last_t: Mutex::new(None),
            writer: None,
        };
        if !enabled {
            return me;
        }
        let opened = path
            .parent()
            .map_or(Ok(()), |d| {
                if d.as_os_str().is_empty() {
                    Ok(())
                } else {
                    std::fs::create_dir_all(d)
                }
            })
            // Fresh file each session so a run is self-contained.
            .and_then(|_| std::fs::write(&path, CSV_HEADER));
        if let Err(e) = opened {
            log::warn!(
                "Scanner HRTF diagnostic could not open '{}': {e}",
                path.display()
            );
            shared.enabled.store(false, Ordering::Relaxed);
            return me;
        }
        let worker = shared.clone();
        me.writer = std::thread::Builder::new()
            .name("scanner-hrtf-diag".into())
            .spawn(move || worker.drain_loop())
            .ok();
        log::info!("Scanner HRTF diagnostic ENABLED -> {}", path.display());
        me
    }

    pub fn enabled(&self) -> bool {
        self.shared.enabled.load(Ordering::Relaxed)
    }

    pub fn path(&self) -> &Path {
        &self.shared.path
    }

    /// Called from the audio callback for each scanner beep frame.
    ///
    /// `ir` is the HRIR pair used (`None` for the stereo-pan fallback); `out_l`/`out_r`
    /// are the samples written this frame. The RMS and log are computed here rather than
    /// on the writer thread so the recorded balance matches the exact samples produced.
    #[allow(clippy::too_many_arguments)]
    pub fn record_beep(
        &self,
        bearing_deg: f64,
        dist_m: f64,
        aligned: bool,
        pitch_norm: f64,
        pitch_mult: f64,
        hrtf_used: bool,
        hrtf_az_deg: f64,
        ir: Option<(&[f64], &[f64])>,
        out_l: &[f32],
        out_r: &[f32],
    ) {
        if !self.enabled() {
            return;
        }
        let now = perf_counter_ms();
        let dt = {
            let mut last = self.last_t.lock();
            let dt = last.map_or(0.0, |l| now - l);
            *last = Some(now);
            dt
        };
        let rms_l = rms_f32(Some(out_l));
        let rms_r = rms_f32(Some(out_r));
        let out_ild = ild_db(rms_l, rms_r);
        let ir_ild = match ir {
            Some((l, r)) => ild_db(rms(Some(l)), rms(Some(r))),
            None => 0.0,
        };
        self.shared.buf.lock().push_back(BeepRow {
            t_ms: now,
            dt_ms: dt,
            bearing_deg,
            dist_m,
            aligned,
            pitch_norm,
            pitch_mult,
            hrtf_used,
            hrtf_az_deg,
            ir_ild_db: ir_ild,
            out_ild_db: out_ild,
            rms_l,
            rms_r,
        });
    }

    /// Write out whatever is queued now (the writer thread does this every 0.25 s).
    pub fn flush(&self) {
        self.shared.flush();
    }

    /// Signal the writer to flush once more and exit (Python's `stop()`; it does not
    /// wait for the thread, and neither does this).
    pub fn stop(&self) {
        *self.shared.stop.lock() = true;
        self.shared.wake.notify_all();
    }

    /// [`stop`](Self::stop), then wait for the final flush (for tests and shutdown).
    pub fn stop_and_join(&mut self) {
        self.stop();
        if let Some(h) = self.writer.take() {
            let _ = h.join();
        }
    }
}
