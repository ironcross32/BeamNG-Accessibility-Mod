//! Procedural audio synthesis, HRTF, the audio controller and the output callback.

pub mod callback;
pub mod consts;
pub mod controller;
pub mod device;
pub mod dsp;
pub mod hrtf;
pub mod npy;
pub mod scan;
pub mod scanner_hrtf_diag;
pub mod waveforms;

pub use callback::clock::{Clock, FakeClock, SystemClock};
pub use controller::AudioController;
pub use device::{list_wasapi_output_devices, play_buffer};
