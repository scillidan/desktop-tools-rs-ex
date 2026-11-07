//! Desktop-audio capture for recording: WASAPI loopback of the default render
//! endpoint, written as 16-bit PCM WAV. ffmpeg's own Windows builds here have
//! no `wasapi` input, so the audio side is ours and ffmpeg only muxes.
//!
//! The worker owns the COM objects and the file; `start` returns once the device
//! is open, `stop` joins it and leaves a playable WAV behind.

use anyhow::{bail, Context, Result};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK,
    WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
const KSDATAFORMAT_SUBTYPE_IEEE_FLOAT: windows::core::GUID =
    windows::core::GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);

pub struct Capture {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    path: PathBuf,
}

impl Capture {
    /// Stops the loopback worker and returns the WAV path (it may be tiny when
    /// the device stayed silent, which the caller treats as "no audio").
    pub fn stop(mut self) -> PathBuf {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        self.path
    }
}

/// Samples already captured, in bytes of PCM when the header is finalized.
pub fn wav_header(data_len: u32, rate: u32, channels: u16, bits: u16) -> Vec<u8> {
    let block = channels * bits / 8;
    let mut h = Vec::with_capacity(44);
    h.extend_from_slice(b"RIFF");
    h.extend_from_slice(&(36 + data_len).to_le_bytes());
    h.extend_from_slice(b"WAVEfmt ");
    h.extend_from_slice(&16u32.to_le_bytes());
    h.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes());
    h.extend_from_slice(&channels.to_le_bytes());
    h.extend_from_slice(&rate.to_le_bytes());
    h.extend_from_slice(&(rate * block as u32).to_le_bytes());
    h.extend_from_slice(&block.to_le_bytes());
    h.extend_from_slice(&bits.to_le_bytes());
    h.extend_from_slice(b"data");
    h.extend_from_slice(&data_len.to_le_bytes());
    h
}

/// Mix-format sample to i16, normalised by the source's full scale.
fn to_i16(sample: &[u8], float: bool, bits: u16) -> i16 {
    if float && bits == 32 && sample.len() >= 4 {
        let v = f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
        return (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    }
    match bits {
        16 if sample.len() >= 2 => i16::from_le_bytes([sample[0], sample[1]]),
        32 if sample.len() >= 4 => {
            let v = i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]);
            (v >> 16) as i16
        }
        24 if sample.len() >= 3 => {
            // 24-bit sits in the top of an i32.
            let v = i32::from_le_bytes([0, sample[0], sample[1], sample[2]]);
            (v >> 16) as i16
        }
        _ => 0,
    }
}

/// Start loopback capture of the default playback device into `path`.
pub fn start(path: &Path) -> Result<Capture> {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let path_owned = path.to_path_buf();
    let flag = stop.clone();
    let join = std::thread::spawn(move || {
        let result = capture_loop(&path_owned, &flag, &tx);
        if let Err(e) = result {
            let _ = tx.send(Err(format!("{e:#}")));
        }
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(Capture {
            stop,
            join: Some(join),
            path: path.to_path_buf(),
        }),
        Ok(Err(e)) => {
            stop.store(true, Ordering::SeqCst);
            let _ = join.join();
            bail!(e)
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            let _ = join.join();
            bail!("audio device did not open in time")
        }
    }
}

fn capture_loop(
    path: &Path,
    stop: &AtomicBool,
    ready: &mpsc::Sender<Result<(), String>>,
) -> Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok().ok();
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .context("CoCreateInstance(MMDeviceEnumerator)")?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .context("no default playback device (nothing to loop back)")?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .context("IAudioClient::Activate")?;
        let format = client
            .GetMixFormat()
            .context("IAudioClient::GetMixFormat")?;
        let wf = *format;
        let tag = wf.wFormatTag;
        let bits = wf.wBitsPerSample;
        let rate = wf.nSamplesPerSec;
        let channels = wf.nChannels;
        let float = tag == WAVE_FORMAT_IEEE_FLOAT
            || (tag == WAVE_FORMAT_EXTENSIBLE
                && std::ptr::read_unaligned(std::ptr::addr_of!(
                    (*(format as *const WAVEFORMATEXTENSIBLE)).SubFormat
                )) == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT);
        if !(bits == 16 || bits == 24 || bits == 32) || channels == 0 || rate == 0 {
            CoTaskMemFree(Some(format as *const _));
            let _ = ready.send(Err(format!(
                "unsupported mix format: {bits} bits, {channels} ch, {rate} Hz"
            )));
            CoUninitialize();
            return Ok(());
        }
        // Shared mode only accepts the endpoint's own mix format.
        let init = client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            10_000_000,
            0,
            format,
            None,
        );
        CoTaskMemFree(Some(format as *const _));
        if let Err(e) = init {
            let _ = ready.send(Err(format!("IAudioClient::Initialize: {e}")));
            CoUninitialize();
            return Ok(());
        }
        let capture: IAudioCaptureClient = client
            .GetService()
            .context("IAudioClient::GetService(IAudioCaptureClient)")?;
        client.Start().context("IAudioClient::Start")?;

        let mut file = match std::fs::File::create(path) {
            Ok(f) => f,
            Err(e) => {
                let _ = ready.send(Err(format!("cannot create {}: {e}", path.display())));
                let _ = client.Stop();
                CoUninitialize();
                return Ok(());
            }
        };
        let _ = file.write_all(&wav_header(0, rate, channels, 16));
        let _ = ready.send(Ok(()));

        let block = wf.nBlockAlign as usize;
        let frame_bytes = block.max(1);
        let mut written: u64 = 0;
        let bytes_per_second = rate as u64 * channels as u64 * 2;
        let began = std::time::Instant::now();
        while !stop.load(Ordering::SeqCst) {
            let mut packet = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(_) => break,
            };
            while packet > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if frames > 0 {
                    let mut out = Vec::with_capacity(frames as usize * channels as usize * 2);
                    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                        out.resize(frames as usize * channels as usize * 2, 0);
                    } else if !data.is_null() {
                        let sample_bytes = (frame_bytes / channels as usize).max(1);
                        for f in 0..frames as usize {
                            for c in 0..channels as usize {
                                let off = f * block + c * sample_bytes;
                                let slice =
                                    std::slice::from_raw_parts(data.add(off), sample_bytes.min(4));
                                out.extend_from_slice(&to_i16(slice, float, bits).to_le_bytes());
                            }
                        }
                    }
                    let _ = file.write_all(&out);
                    written += out.len() as u64;
                }
                let _ = capture.ReleaseBuffer(frames);
                packet = capture.GetNextPacketSize().unwrap_or(0);
            }
            // WASAPI loopback only delivers packets while something plays.
            // Pad the gap with silence so the track stays aligned with the
            // video instead of being shorter than it.
            let expected = began.elapsed().as_millis() as u64 * bytes_per_second / 1000;
            if expected > written {
                let pad = (expected - written).min(bytes_per_second / 4) as usize;
                let _ = file.write_all(&vec![0u8; pad]);
                written += pad as u64;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = client.Stop();
        // Patch the RIFF sizes now that the payload length is known.
        if written > 0 {
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.write_all(&wav_header(
                written.min(u32::MAX as u64) as u32,
                rate,
                channels,
                16,
            ));
        }
        let _ = file.flush();
        CoUninitialize();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_a_pcm_wav() {
        let h = wav_header(1000, 48000, 2, 16);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes([h[4], h[5], h[6], h[7]]), 36 + 1000);
        assert_eq!(&h[8..12], b"WAVE");
        assert_eq!(u16::from_le_bytes([h[20], h[21]]), WAVE_FORMAT_PCM);
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), 48000);
        assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
        assert_eq!(&h[36..40], b"data");
        assert_eq!(h.len(), 44);
    }

    #[test]
    fn samples_convert_from_every_mix_width() {
        assert_eq!(to_i16(&(-1.0f32).to_le_bytes(), true, 32), -32767);
        assert_eq!(to_i16(&(1.0f32).to_le_bytes(), true, 32), 32767);
        assert_eq!(to_i16(&(0.5f32).to_le_bytes(), true, 32), 16384);
        assert_eq!(to_i16(&1234i16.to_le_bytes(), false, 16), 1234);
        assert_eq!(to_i16(&(0x4000_0000i32).to_le_bytes(), false, 32), 0x4000);
        assert_eq!(to_i16(&[0x00, 0x00, 0x40], false, 24), 0x4000);
        assert_eq!(to_i16(&[], true, 32), 0);
    }
}
