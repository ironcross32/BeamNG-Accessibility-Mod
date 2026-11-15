//! Output device handling (audio.py:7025-7235) on cpal's WASAPI host: host-API lookup,
//! target device selection (follow the OS default, or a fixed device by name with
//! fallback to the default), stream (re)start, the default-device watcher thread, and
//! start/stop. Plus the two configurator helpers: the sorted WASAPI output device names
//! (`configurator.list_wasapi_output_devices`) and a fire-and-forget buffer player
//! (`sd.play(buffer, samplerate, blocking=False)`).
//!
//! Device "index" is the position in the WASAPI host's device enumeration, enumerated
//! afresh on every query. PortAudio's indices span every host API; cpal's WASAPI list is
//! the subset Python filtered down to, so the selection rules are unchanged.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, Device, Host, HostId, SampleRate, Stream, StreamConfig};
use parking_lot::Mutex;

use crate::controller::AudioController;

/// `_find_hostapi_index("wasapi")`: the WASAPI host, or `None` when it cannot be opened.
pub fn find_wasapi_host() -> Option<Host> {
    match cpal::host_from_id(HostId::Wasapi) {
        Ok(h) => Some(h),
        Err(e) => {
            log::warn!("query_hostapis failed: {e}");
            None
        }
    }
}

/// Every device the host enumerates, in order (Python's `sd.query_devices()`).
fn all_devices(host: &Host) -> Vec<Device> {
    match host.devices() {
        Ok(it) => it.collect(),
        Err(_) => Vec::new(),
    }
}

/// `device["max_output_channels"] > 0`.
fn has_output(device: &Device) -> bool {
    device.supports_output()
}

/// The device's name (`device["name"]`).
fn name_of(device: &Device) -> String {
    match device.description() {
        Ok(d) => d.name().to_string(),
        Err(_) => device.to_string(),
    }
}

/// `sd.default.device[1]`: the index of the OS default output device, or `None` (-1).
fn default_output_index(host: &Host, devices: &[Device]) -> Option<usize> {
    let default = host.default_output_device()?;
    let default_id = default.id().ok();
    let default_name = name_of(&default);
    devices
        .iter()
        .position(|d| match (&default_id, d.id().ok()) {
            (Some(a), Some(b)) => *a == b,
            _ => name_of(d) == default_name,
        })
}

/// `configurator.list_wasapi_output_devices()`: sorted WASAPI output device names.
pub fn list_wasapi_output_devices() -> Vec<String> {
    let Ok(host) = cpal::host_from_id(HostId::Wasapi) else {
        return Vec::new();
    };
    let mut names: Vec<String> = all_devices(&host)
        .iter()
        .filter(|d| has_output(d))
        .map(name_of)
        .collect();
    names.sort();
    names
}

/// The stream `play_buffer` keeps alive, like sounddevice's module-level `_last_callback`.
static PLAY_STREAM: Mutex<Option<Stream>> = Mutex::new(None);

/// `sd.play(buffer, samplerate=sr, blocking=False)`: play a mono buffer once on the
/// default output device, stopping whatever the previous call was playing. Returns the
/// error text where Python would raise.
pub fn play_buffer(buffer: Vec<f32>, samplerate: u32) -> Result<(), String> {
    // A new play() stops the previous one first, as sounddevice does.
    let previous = PLAY_STREAM.lock().take();
    drop(previous);

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| "No default output device".to_string())?;
    let data = Arc::new(buffer);
    // Mono first (what sounddevice opens); a device that refuses one channel gets the
    // buffer duplicated across its default channel count.
    let mut attempts: Vec<u16> = vec![1];
    if let Ok(cfg) = device.default_output_config() {
        if cfg.channels() != 1 {
            attempts.push(cfg.channels());
        }
    }
    let mut last_err = String::new();
    for channels in attempts {
        let config = StreamConfig {
            channels,
            sample_rate: samplerate as SampleRate,
            buffer_size: BufferSize::Default,
        };
        let data = data.clone();
        let mut pos = 0usize;
        let ch = channels as usize;
        let built = device.build_output_stream::<f32, _, _>(
            config,
            move |out: &mut [f32], _| {
                for frame in out.chunks_mut(ch) {
                    let x = data.get(pos).copied().unwrap_or(0.0);
                    for s in frame.iter_mut() {
                        *s = x;
                    }
                    pos += 1;
                }
            },
            |e| log::warn!("Playback stream error: {e}"),
            None,
        );
        match built {
            Ok(stream) => {
                stream.play().map_err(|e| e.to_string())?;
                *PLAY_STREAM.lock() = Some(stream);
                return Ok(());
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(last_err)
}

impl AudioController {
    /// `_find_target_device(verbose)`: the device index to open, or `None`.
    pub fn find_target_device(&self, verbose: bool) -> Option<usize> {
        let (follow, preferred) = {
            let s = self.lock.lock();
            (s._follow_default_enabled, s._preferred_device_name.clone())
        };
        let wasapi = find_wasapi_host();
        // Python's fallback "system default" is PortAudio's default host API; on Windows
        // cpal's default host is WASAPI itself.
        let fallback_host;
        let host = match wasapi {
            Some(ref h) => h,
            None => {
                fallback_host = cpal::default_host();
                &fallback_host
            }
        };
        let devices = all_devices(host);
        let system_default = default_output_index(host, &devices);

        if !follow && !preferred.is_empty() {
            // Fixed-device mode: open the named WASAPI device directly.
            if wasapi.is_none() {
                log::warn!("WASAPI not available; falling back to system default.");
                return system_default;
            }
            for (idx, device) in devices.iter().enumerate() {
                if has_output(device) && name_of(device).contains(&preferred) {
                    if verbose {
                        log::info!(
                            "Using fixed WASAPI device: '{}' (Index: {})",
                            name_of(device),
                            idx
                        );
                    }
                    return Some(idx);
                }
            }
            log::warn!(
                "Fixed device '{}' not found; falling back to system default.",
                preferred
            );
            return system_default;
        }

        // Follow-default mode: the WASAPI device matching the OS default.
        let Some(system_default_idx) = system_default else {
            log::warn!("No default output device found in the system.");
            return None;
        };
        let default_name = name_of(&devices[system_default_idx]);
        if verbose {
            log::info!(
                "System default output device is: '{}' (Index: {})",
                default_name,
                system_default_idx
            );
        }
        if wasapi.is_none() {
            log::warn!("WASAPI not found; using system default device.");
            return Some(system_default_idx);
        }
        for (idx, device) in devices.iter().enumerate() {
            if has_output(device) && name_of(device).contains(&default_name) {
                if verbose {
                    log::info!(
                        "Found matching WASAPI device: '{}' (Index: {})",
                        name_of(device),
                        idx
                    );
                }
                return Some(idx);
            }
        }
        log::warn!(
            "No WASAPI equivalent for '{}'; falling back to system default.",
            default_name
        );
        Some(system_default_idx)
    }

    /// `_device_name(idx)`.
    pub fn device_name(&self, idx: usize) -> String {
        let host = find_wasapi_host().unwrap_or_else(cpal::default_host);
        all_devices(&host)
            .get(idx)
            .map(name_of)
            .unwrap_or_else(|| "Unknown Device".to_string())
    }

    /// `_restart_audio_stream(new_device_index)`. The old stream is torn down OUTSIDE the
    /// lock: dropping a cpal stream waits for its callback, and the callback takes the
    /// lock, so holding it across the drop deadlocks the two.
    pub fn restart_audio_stream(self: &Arc<Self>, new_device_index: usize) {
        let old_stream = {
            let mut s = self.lock.lock();
            if s._current_device_index == Some(new_device_index) && s._audio_stream.is_some() {
                return;
            }
            s._audio_stream.take()
        };
        drop(old_stream);

        let result = (|| -> Result<(Stream, String), String> {
            let host = find_wasapi_host().unwrap_or_else(cpal::default_host);
            let devices = all_devices(&host);
            let device = devices
                .get(new_device_index)
                .ok_or_else(|| format!("Invalid device index {new_device_index}"))?;
            let default_cfg = device.default_output_config().map_err(|e| e.to_string())?;
            let new_samplerate = default_cfg.sample_rate() as f64;

            // No stream is running here, so nothing can be reading these.
            let changed = {
                let mut s = self.lock.lock();
                if s.samplerate != new_samplerate {
                    s.samplerate = new_samplerate;
                    true
                } else {
                    false
                }
            };
            if changed {
                self.regenerate_waveforms();
                let mut s = self.lock.lock();
                s._phase_inc_shift = s.shift_freq / s.samplerate;
            }

            let sr = self.samplerate();
            let config = StreamConfig {
                channels: 2,
                sample_rate: sr as SampleRate,
                // sounddevice's default blocksize=0: let the host choose.
                buffer_size: BufferSize::Default,
            };
            let weak = Arc::downgrade(self);
            let stream = device
                .build_output_stream::<f32, _, _>(
                    config,
                    move |out: &mut [f32], _| match weak.upgrade() {
                        Some(ctrl) => {
                            let frames = out.len() / 2;
                            ctrl.audio_callback(out, frames);
                        }
                        None => out.fill(0.0),
                    },
                    |e| log::warn!("Audio stream error: {e}"),
                    None,
                )
                .map_err(|e| e.to_string())?;
            // play() outside the lock: the first callback wants the lock immediately.
            stream.play().map_err(|e| e.to_string())?;
            Ok((stream, name_of(device)))
        })();

        match result {
            Ok((stream, name)) => {
                let sr = {
                    let mut s = self.lock.lock();
                    s._audio_stream = Some(stream);
                    s._current_device_index = Some(new_device_index);
                    s._current_device_name = Some(name.clone());
                    s.samplerate
                };
                // A restart clears a previous block's abort latch (a fresh PortAudio stream).
                self.voice.lock().callback_aborted = false;
                log::info!(
                    "Audio stream started on device: '{}' at {} Hz.",
                    name,
                    sr.trunc() as i64
                );
            }
            Err(e) => {
                log::error!(
                    "Failed to start audio stream on device index {}: {}",
                    new_device_index,
                    e
                );
                let mut s = self.lock.lock();
                s._audio_stream = None;
                s._current_device_index = None;
            }
        }
    }

    /// `_device_watcher_loop`: every `_audio_poll_interval` seconds, restart on the new
    /// target when it differs from the current device.
    pub fn device_watcher_loop(self: &Arc<Self>) {
        while !self._stop_event.is_set() {
            let interval = self.lock.lock()._audio_poll_interval;
            self._stop_event.wait(interval);
            if self._stop_event.is_set() {
                break;
            }
            let target = self.find_target_device(false);
            let current = self.lock.lock()._current_device_index;
            if let Some(t) = target {
                if Some(t) != current {
                    let new_name = self.device_name(t);
                    log::info!(
                        "Default audio device appears to have changed to: '{}'. Restarting stream.",
                        new_name
                    );
                    self.find_target_device(true);
                    self.restart_audio_stream(t);
                }
            }
        }
    }

    /// Start the watcher thread and record its handle.
    pub fn spawn_device_watcher(self: &Arc<Self>) {
        let me = self.clone();
        let handle = std::thread::Builder::new()
            .name("audio-device-watcher".into())
            .spawn(move || me.device_watcher_loop());
        match handle {
            Ok(h) => self.lock.lock()._device_watcher_thread = Some(h),
            Err(e) => log::warn!("Could not start the audio device watcher: {e}"),
        }
    }

    /// `start()`: pick the device, generate the waveforms, open the stream, and follow the
    /// OS default if configured.
    pub fn start(self: &Arc<Self>) {
        if !self.is_enabled() {
            return;
        }
        self._stop_event.clear();
        let Some(initial) = self.find_target_device(true) else {
            log::error!("No suitable output audio device found. Audio will be disabled.");
            self._is_enabled
                .store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        self.regenerate_waveforms();
        self.restart_audio_stream(initial);
        if self.lock.lock()._follow_default_enabled {
            self.spawn_device_watcher();
            log::info!("Following OS default output device (WASAPI).");
        }
    }

    /// `stop()`.
    pub fn stop(&self) {
        if !self.is_enabled() {
            return;
        }
        self._stop_event.set();
        let (watcher, interval) = {
            let mut s = self.lock.lock();
            (s._device_watcher_thread.take(), s._audio_poll_interval)
        };
        if let Some(h) = watcher {
            // join(timeout=_audio_poll_interval); a thread still busy after that is left
            // to finish on its own, like Python's daemon thread.
            let deadline = Instant::now() + Duration::from_secs_f64(interval.max(0.0).min(3600.0));
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        // Outside the lock, as in restart_audio_stream.
        let stream = self.lock.lock()._audio_stream.take();
        drop(stream);
        let diag = self.voice.lock()._scanner_diag.clone();
        if let Some(d) = diag {
            d.stop();
        }
        log::info!("Audio system stopped.");
    }
}
