//! WASAPI loopback capture — captures system audio output.
//!
//! Uses process-exclusion loopback (Windows 10 2004+) to capture all system
//! audio EXCEPT audio played by our own process, preventing feedback loops
//! when screen sharing. Converts to 48 kHz stereo 16-bit PCM for Dart.

use std::sync::{Arc, Condvar, Mutex, OnceLock};

use windows::core::{implement, IUnknown, Interface, PROPVARIANT};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::GetCurrentProcessId;

// WAVE_FORMAT constants not always exported by the windows crate
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;

// ── Output format: 48 kHz stereo 16-bit PCM ─────────────────────────────

const OUT_SAMPLE_RATE: u32 = 48000;
const OUT_CHANNELS: u16 = 2;
const OUT_BITS: u16 = 16;
const OUT_BLOCK_ALIGN: u16 = OUT_CHANNELS * (OUT_BITS / 8);
const _OUT_AVG_BYTES: u32 = OUT_SAMPLE_RATE * OUT_BLOCK_ALIGN as u32;

// ── Global state ─────────────────────────────────────────────────────────

struct LoopbackState {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    /// Ring buffer for converted PCM
    buffer: Vec<u8>,
    /// Source format info for conversion
    src_channels: u16,
    src_sample_rate: u32,
    src_bits_per_sample: u16,
    src_is_float: bool,
}

// Safety: COM pointers are thread-safe when properly marshaled.
// We only access from one thread at a time via Mutex.
unsafe impl Send for LoopbackState {}

static LOOPBACK: OnceLock<Arc<Mutex<Option<LoopbackState>>>> = OnceLock::new();

fn global() -> &'static Arc<Mutex<Option<LoopbackState>>> {
    LOOPBACK.get_or_init(|| Arc::new(Mutex::new(None)))
}

// ── Public FFI ───────────────────────────────────────────────────────────

/// Start loopback capture. Returns 0 on success, -1 on error.
pub fn start() -> i32 {
    let mut guard = global().lock().unwrap();
    if guard.is_some() {
        return 0; // already running
    }

    match init_loopback() {
        Ok(state) => {
            *guard = Some(state);
            0
        }
        Err(e) => {
            eprintln!("loopback_start failed: {e}");
            -1
        }
    }
}

/// Poll captured audio. Copies up to `max_len` bytes of 48kHz stereo 16-bit PCM
/// into `buf`. Returns number of bytes written, or -1 on error.
pub fn poll(buf: *mut u8, max_len: u32) -> i32 {
    let mut guard = global().lock().unwrap();
    let state = match guard.as_mut() {
        Some(s) => s,
        None => return -1,
    };

    // Drain WASAPI capture buffer into our ring buffer
    if let Err(e) = drain_capture(state) {
        eprintln!("loopback drain error: {e}");
        return -1;
    }

    let available = state.buffer.len().min(max_len as usize);
    if available == 0 {
        return 0;
    }

    unsafe {
        std::ptr::copy_nonoverlapping(state.buffer.as_ptr(), buf, available);
    }
    state.buffer.drain(..available);

    available as i32
}

/// Stop loopback capture. Returns 0 on success.
pub fn stop() -> i32 {
    let mut guard = global().lock().unwrap();
    if let Some(state) = guard.take() {
        unsafe {
            let _ = state.client.Stop();
        }
    }
    0
}

// ── Internal ─────────────────────────────────────────────────────────────

// ── Completion handler for ActivateAudioInterfaceAsync ──────────────────

/// Shared state signaled by the completion handler when activation finishes.
struct ActivationResult {
    done: bool,
    hr: i32,
    interface: Option<IUnknown>,
}

/// COM object implementing IActivateAudioInterfaceCompletionHandler.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct CompletionHandler {
    state: Arc<(Mutex<ActivationResult>, Condvar)>,
}

impl IActivateAudioInterfaceCompletionHandler_Impl for CompletionHandler_Impl {
    fn ActivateCompleted(
        &self,
        operation: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        let (lock, cvar) = &*self.state;
        let mut result = lock.lock().unwrap();

        if let Some(op) = operation {
            let mut hr = windows::core::HRESULT(0);
            let mut activated: Option<IUnknown> = None;
            unsafe {
                let _ = op.GetActivateResult(&mut hr, &mut activated);
            }
            result.hr = hr.0;
            result.interface = activated;
        } else {
            result.hr = -1;
        }
        result.done = true;
        cvar.notify_one();
        Ok(())
    }
}

// VT_BLOB = 65 (from Win32_System_Variant, hardcoded to avoid extra feature dep)
const VT_BLOB: u16 = 65;

fn init_loopback() -> Result<LoopbackState, String> {
    unsafe {
        // Initialize COM (may already be initialized — that's OK)
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        // Build process-exclusion loopback activation params
        let loopback_params = AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
            TargetProcessId: GetCurrentProcessId(),
            ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
        };

        let mut activation_params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: loopback_params,
            },
        };

        // Wrap in a PROPVARIANT (VT_BLOB pointing to activation_params)
        let params_ptr = &mut activation_params as *mut _ as *mut u8;
        let params_size = std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32;

        let mut propvariant: windows::core::imp::PROPVARIANT = std::mem::zeroed();
        propvariant.Anonymous.Anonymous.vt = VT_BLOB;
        propvariant.Anonymous.Anonymous.Anonymous.blob.cbSize = params_size;
        propvariant.Anonymous.Anonymous.Anonymous.blob.pBlobData = params_ptr;
        let propvariant = PROPVARIANT::from_raw(propvariant);

        // Create completion handler
        let pair = Arc::new((
            Mutex::new(ActivationResult {
                done: false,
                hr: 0,
                interface: None,
            }),
            Condvar::new(),
        ));

        let handler: IActivateAudioInterfaceCompletionHandler =
            CompletionHandler { state: pair.clone() }.into();

        // Activate with process-exclusion loopback
        let _op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&propvariant),
            &handler,
        )
        .map_err(|e| format!("ActivateAudioInterfaceAsync: {e}"))?;

        // Wait for completion
        let (lock, cvar) = &*pair;
        let result = cvar
            .wait_while(lock.lock().unwrap(), |r| !r.done)
            .unwrap();

        if result.hr < 0 {
            return Err(format!(
                "Process loopback activation failed: HRESULT 0x{:08X}",
                result.hr as u32
            ));
        }

        let activated = result
            .interface
            .as_ref()
            .ok_or("Activation returned no interface")?;

        let client: IAudioClient = activated
            .cast()
            .map_err(|e| format!("Cast to IAudioClient: {e}"))?;

        // Get device mix format
        let mix_format_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat: {e}"))?;
        let mix_format = &*mix_format_ptr;

        let src_channels = mix_format.nChannels;
        let src_sample_rate = mix_format.nSamplesPerSec;
        let src_bits_per_sample = mix_format.wBitsPerSample;
        let src_is_float = mix_format.wFormatTag == WAVE_FORMAT_IEEE_FLOAT_TAG
            || (mix_format.wFormatTag == WAVE_FORMAT_EXTENSIBLE_TAG && src_bits_per_sample == 32);

        // Initialize — NO LOOPBACK flag (mode already set via activation params)
        let buffer_duration = 200_000; // 20ms in 100ns units
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                0, // no flags — process loopback mode set at activation
                buffer_duration,
                0, // periodicity (must be 0 for shared mode)
                mix_format_ptr,
                None,
            )
            .map_err(|e| format!("Initialize loopback: {e}"))?;

        CoTaskMemFree(Some(mix_format_ptr as *const _ as *const _));

        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| format!("GetService IAudioCaptureClient: {e}"))?;

        client
            .Start()
            .map_err(|e| format!("Start capture: {e}"))?;

        Ok(LoopbackState {
            client,
            capture,
            buffer: Vec::with_capacity(48000 * 4), // ~1s buffer
            src_channels,
            src_sample_rate,
            src_bits_per_sample,
            src_is_float,
        })
    }
}

/// Drain all available packets from WASAPI capture into our conversion buffer.
fn drain_capture(state: &mut LoopbackState) -> Result<(), String> {
    unsafe {
        loop {
            let packet_size = state
                .capture
                .GetNextPacketSize()
                .map_err(|e| format!("GetNextPacketSize: {e}"))?;

            if packet_size == 0 {
                break;
            }

            let mut data_ptr = std::ptr::null_mut();
            let mut num_frames = 0u32;
            let mut flags = 0u32;

            state
                .capture
                .GetBuffer(&mut data_ptr, &mut num_frames, &mut flags, None, None)
                .map_err(|e| format!("GetBuffer: {e}"))?;

            if num_frames > 0 && !data_ptr.is_null() {
                let silent = (flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32)) != 0;

                if silent {
                    // Write silence in output format
                    let out_frames = resample_frame_count(
                        num_frames,
                        state.src_sample_rate,
                        OUT_SAMPLE_RATE,
                    );
                    let silence_bytes = out_frames as usize * OUT_BLOCK_ALIGN as usize;
                    state.buffer.extend(std::iter::repeat(0u8).take(silence_bytes));
                } else {
                    // Convert source → 48kHz stereo 16-bit PCM
                    let src_frame_bytes =
                        state.src_channels as usize * (state.src_bits_per_sample as usize / 8);
                    let src_bytes = num_frames as usize * src_frame_bytes;
                    let src_slice = std::slice::from_raw_parts(data_ptr, src_bytes);

                    convert_audio(
                        src_slice,
                        state.src_channels,
                        state.src_sample_rate,
                        state.src_bits_per_sample,
                        state.src_is_float,
                        &mut state.buffer,
                    );
                }
            }

            state
                .capture
                .ReleaseBuffer(num_frames)
                .map_err(|e| format!("ReleaseBuffer: {e}"))?;
        }
    }
    Ok(())
}

/// Convert source audio to 48kHz stereo 16-bit PCM and append to output buffer.
fn convert_audio(
    src: &[u8],
    src_channels: u16,
    src_sample_rate: u32,
    src_bits: u16,
    src_is_float: bool,
    out: &mut Vec<u8>,
) {
    let src_frame_bytes = src_channels as usize * (src_bits as usize / 8);
    let num_src_frames = src.len() / src_frame_bytes;
    if num_src_frames == 0 {
        return;
    }

    // Step 1: Extract source frames as f32 stereo samples
    let mut src_stereo: Vec<[f32; 2]> = Vec::with_capacity(num_src_frames);

    for i in 0..num_src_frames {
        let frame_start = i * src_frame_bytes;
        let (left, right) = read_stereo_frame(
            &src[frame_start..frame_start + src_frame_bytes],
            src_channels,
            src_bits,
            src_is_float,
        );
        src_stereo.push([left, right]);
    }

    // Step 2: Resample to 48kHz if needed
    if src_sample_rate == OUT_SAMPLE_RATE {
        // No resampling needed — direct conversion to i16
        for [left, right] in &src_stereo {
            let l = f32_to_i16(*left);
            let r = f32_to_i16(*right);
            out.extend_from_slice(&l.to_le_bytes());
            out.extend_from_slice(&r.to_le_bytes());
        }
    } else {
        // Linear interpolation resampling
        let ratio = src_sample_rate as f64 / OUT_SAMPLE_RATE as f64;
        let out_frames = resample_frame_count(
            num_src_frames as u32,
            src_sample_rate,
            OUT_SAMPLE_RATE,
        );

        for i in 0..out_frames {
            let src_pos = i as f64 * ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f64;

            let [l0, r0] = if idx < src_stereo.len() {
                src_stereo[idx]
            } else {
                [0.0, 0.0]
            };
            let [l1, r1] = if idx + 1 < src_stereo.len() {
                src_stereo[idx + 1]
            } else {
                [l0, r0]
            };

            let left = l0 + (l1 - l0) * frac as f32;
            let right = r0 + (r1 - r0) * frac as f32;

            out.extend_from_slice(&f32_to_i16(left).to_le_bytes());
            out.extend_from_slice(&f32_to_i16(right).to_le_bytes());
        }
    }
}

/// Read one frame from source data and return as stereo f32 [-1.0, 1.0].
fn read_stereo_frame(frame: &[u8], channels: u16, bits: u16, is_float: bool) -> (f32, f32) {
    let sample_bytes = bits as usize / 8;

    let read_sample = |offset: usize| -> f32 {
        if is_float && bits == 32 {
            f32::from_le_bytes(frame[offset..offset + 4].try_into().unwrap())
        } else if bits == 16 {
            let val = i16::from_le_bytes(frame[offset..offset + 2].try_into().unwrap());
            val as f32 / 32768.0
        } else if bits == 24 {
            let b = [frame[offset], frame[offset + 1], frame[offset + 2], 0];
            let val = i32::from_le_bytes(b) >> 8;
            val as f32 / 8388608.0
        } else if bits == 32 && !is_float {
            let val = i32::from_le_bytes(frame[offset..offset + 4].try_into().unwrap());
            val as f32 / 2147483648.0
        } else {
            0.0
        }
    };

    let left = read_sample(0);
    let right = if channels >= 2 {
        read_sample(sample_bytes)
    } else {
        left // mono → duplicate to stereo
    };

    (left, right)
}

fn f32_to_i16(val: f32) -> i16 {
    let clamped = val.clamp(-1.0, 1.0);
    if clamped < 0.0 {
        (clamped * 32768.0) as i16
    } else {
        (clamped * 32767.0) as i16
    }
}

fn resample_frame_count(src_frames: u32, src_rate: u32, out_rate: u32) -> u32 {
    ((src_frames as u64 * out_rate as u64) / src_rate as u64) as u32
}
