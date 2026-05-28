//! Diagnostic: capture WASAPI system-audio loopback for 4 seconds and report
//! how much real (non-zero) audio arrived. Run it while something is playing
//! through the default output device to verify the loopback path end to end.
//! Uses the exact same windows-crate call sequence as `src/audio.rs`.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, WAVEFORMATEXTENSIBLE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).unwrap();
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).unwrap();

        let pwfx = client.GetMixFormat().unwrap();
        let wfx = &*pwfx;
        let in_rate = wfx.nSamplesPerSec;
        let channels = wfx.nChannels as usize;
        let bits = wfx.wBitsPerSample as usize;
        let block_align = wfx.nBlockAlign as usize;
        let tag = wfx.wFormatTag;
        let is_float = tag == 0x0003
            || (tag == 0xFFFE && (*(pwfx as *const WAVEFORMATEXTENSIBLE)).SubFormat.data1 == 3);
        println!(
            "mix format: {in_rate} Hz, {channels} ch, {bits} bit, float={is_float}, block_align={block_align}"
        );

        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                2_000_000,
                0,
                pwfx,
                None,
            )
            .unwrap();
        CoTaskMemFree(Some(pwfx as *const c_void));

        let capture: IAudioCaptureClient = client.GetService().unwrap();
        client.Start().unwrap();

        let mut total_frames: u64 = 0;
        let mut nonzero: u64 = 0;
        let mut peak: f32 = 0.0;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(4) {
            loop {
                let packet = capture.GetNextPacketSize().unwrap();
                if packet == 0 {
                    break;
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .unwrap();
                if frames > 0 && !data.is_null() && (flags & 0x2) == 0 {
                    let raw = std::slice::from_raw_parts(data, frames as usize * block_align);
                    for f in 0..frames as usize {
                        let off = f * block_align;
                        let v = if is_float && bits == 32 {
                            f32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
                        } else if bits == 16 {
                            i16::from_le_bytes([raw[off], raw[off + 1]]) as f32 / 32_768.0
                        } else if bits == 32 {
                            i32::from_le_bytes([raw[off], raw[off + 1], raw[off + 2], raw[off + 3]])
                                as f32
                                / 2_147_483_648.0
                        } else {
                            0.0
                        };
                        if v.abs() > 1e-5 {
                            nonzero += 1;
                        }
                        if v.abs() > peak {
                            peak = v.abs();
                        }
                    }
                }
                total_frames += frames as u64;
                capture.ReleaseBuffer(frames).unwrap();
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        let _ = client.Stop();
        CoUninitialize();

        println!("RESULT total_frames={total_frames} nonzero_samples={nonzero} peak={peak:.5}");
        if nonzero > 0 {
            println!("LOOPBACK_OK");
        } else {
            println!("LOOPBACK_SILENT (was anything playing?)");
        }
    }
}
