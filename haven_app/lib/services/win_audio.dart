import 'dart:ffi';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';

// ── Constants ──

const int _waveFormatPcm = 0x0001;
const int _callbackNull = 0x00000000;
const int _waveMapper = 0xFFFFFFFF;
const int _whdrDone = 0x00000001;
const int _mmsyserrNoerror = 0;

// ── Structs ──

@Packed(1)
final class WAVEFORMATEX extends Struct {
  @Uint16()
  external int wFormatTag;
  @Uint16()
  external int nChannels;
  @Uint32()
  external int nSamplesPerSec;
  @Uint32()
  external int nAvgBytesPerSec;
  @Uint16()
  external int nBlockAlign;
  @Uint16()
  external int wBitsPerSample;
  @Uint16()
  external int cbSize;
}

final class WAVEHDR extends Struct {
  external Pointer<Uint8> lpData;
  @Uint32()
  external int dwBufferLength;
  @Uint32()
  external int dwBytesRecorded;
  @IntPtr()
  external int dwUser;
  @Uint32()
  external int dwFlags;
  @Uint32()
  external int dwLoops;
  external Pointer<WAVEHDR> lpNext;
  @IntPtr()
  external int reserved;
}

// ── Native function typedefs ──

// waveIn
typedef _WaveInOpenN = Uint32 Function(Pointer<IntPtr> phwi, Uint32 uDeviceID,
    Pointer<WAVEFORMATEX> pwfx, IntPtr dwCallback, IntPtr dwInst, Uint32 fdw);
typedef _WaveInOpenD = int Function(Pointer<IntPtr> phwi, int uDeviceID,
    Pointer<WAVEFORMATEX> pwfx, int dwCallback, int dwInst, int fdw);

typedef _WaveInHdrN = Uint32 Function(
    IntPtr hwi, Pointer<WAVEHDR> pwh, Uint32 cbwh);
typedef _WaveInHdrD = int Function(
    int hwi, Pointer<WAVEHDR> pwh, int cbwh);

typedef _WaveInSimpleN = Uint32 Function(IntPtr hwi);
typedef _WaveInSimpleD = int Function(int hwi);

// waveOut
typedef _WaveOutOpenN = Uint32 Function(
    Pointer<IntPtr> phwo,
    Uint32 uDeviceID,
    Pointer<WAVEFORMATEX> pwfx,
    IntPtr dwCallback,
    IntPtr dwInst,
    Uint32 fdw);
typedef _WaveOutOpenD = int Function(Pointer<IntPtr> phwo, int uDeviceID,
    Pointer<WAVEFORMATEX> pwfx, int dwCallback, int dwInst, int fdw);

typedef _WaveOutHdrN = Uint32 Function(
    IntPtr hwo, Pointer<WAVEHDR> pwh, Uint32 cbwh);
typedef _WaveOutHdrD = int Function(
    int hwo, Pointer<WAVEHDR> pwh, int cbwh);

typedef _WaveOutSimpleN = Uint32 Function(IntPtr hwo);
typedef _WaveOutSimpleD = int Function(int hwo);

// ── Load winmm.dll ──

final DynamicLibrary _winmm = DynamicLibrary.open('winmm.dll');

final _waveInOpen =
    _winmm.lookupFunction<_WaveInOpenN, _WaveInOpenD>('waveInOpen');
final _waveInClose =
    _winmm.lookupFunction<_WaveInSimpleN, _WaveInSimpleD>('waveInClose');
final _waveInStart =
    _winmm.lookupFunction<_WaveInSimpleN, _WaveInSimpleD>('waveInStart');
// waveInStop not needed — waveInReset stops and returns all buffers.
final _waveInReset =
    _winmm.lookupFunction<_WaveInSimpleN, _WaveInSimpleD>('waveInReset');
final _waveInPrepareHeader =
    _winmm.lookupFunction<_WaveInHdrN, _WaveInHdrD>('waveInPrepareHeader');
final _waveInUnprepareHeader =
    _winmm.lookupFunction<_WaveInHdrN, _WaveInHdrD>('waveInUnprepareHeader');
final _waveInAddBuffer =
    _winmm.lookupFunction<_WaveInHdrN, _WaveInHdrD>('waveInAddBuffer');

final _waveOutOpen =
    _winmm.lookupFunction<_WaveOutOpenN, _WaveOutOpenD>('waveOutOpen');
final _waveOutClose =
    _winmm.lookupFunction<_WaveOutSimpleN, _WaveOutSimpleD>('waveOutClose');
final _waveOutReset =
    _winmm.lookupFunction<_WaveOutSimpleN, _WaveOutSimpleD>('waveOutReset');
final _waveOutPrepareHeader =
    _winmm.lookupFunction<_WaveOutHdrN, _WaveOutHdrD>('waveOutPrepareHeader');
final _waveOutUnprepareHeader = _winmm
    .lookupFunction<_WaveOutHdrN, _WaveOutHdrD>('waveOutUnprepareHeader');
final _waveOutWrite =
    _winmm.lookupFunction<_WaveOutHdrN, _WaveOutHdrD>('waveOutWrite');

// ── Helper: fill WAVEFORMATEX for 16kHz mono 16-bit PCM ──

Pointer<WAVEFORMATEX> _makeFormat() {
  final fmt = calloc<WAVEFORMATEX>();
  fmt.ref.wFormatTag = _waveFormatPcm;
  fmt.ref.nChannels = 1;
  fmt.ref.nSamplesPerSec = 16000;
  fmt.ref.wBitsPerSample = 16;
  fmt.ref.nBlockAlign = 2; // channels * bitsPerSample / 8
  fmt.ref.nAvgBytesPerSec = 16000 * 2;
  fmt.ref.cbSize = 0;
  return fmt;
}

// ══════════════════════════════════════════════════════════════════════
//  WinAudioCapture — microphone input via waveIn
// ══════════════════════════════════════════════════════════════════════

class WinAudioCapture {
  static const int _numBufs = 8;

  /// 320 samples * 2 bytes = 640 bytes = 20 ms at 16 kHz.
  static const int _bufBytes = 640;

  int _hWaveIn = 0;
  bool _active = false;
  final Pointer<IntPtr> _hPtr = calloc<IntPtr>();
  final Pointer<WAVEFORMATEX> _fmt = _makeFormat();
  final List<Pointer<WAVEHDR>> _hdrs = [];
  final List<Pointer<Uint8>> _bufs = [];

  bool get isActive => _active;

  /// Open the default recording device and start capturing.
  void start() {
    if (_active) return;

    final r = _waveInOpen(_hPtr, _waveMapper, _fmt, 0, 0, _callbackNull);
    if (r != _mmsyserrNoerror) {
      throw AudioException('waveInOpen failed (code $r). No microphone?');
    }
    _hWaveIn = _hPtr.value;

    // Allocate and queue buffers
    for (int i = 0; i < _numBufs; i++) {
      final buf = calloc<Uint8>(_bufBytes);
      final hdr = calloc<WAVEHDR>();
      hdr.ref.lpData = buf;
      hdr.ref.dwBufferLength = _bufBytes;
      hdr.ref.dwBytesRecorded = 0;
      hdr.ref.dwFlags = 0;
      hdr.ref.dwLoops = 0;

      _check(_waveInPrepareHeader(_hWaveIn, hdr, sizeOf<WAVEHDR>()),
          'waveInPrepareHeader');
      _check(
          _waveInAddBuffer(_hWaveIn, hdr, sizeOf<WAVEHDR>()), 'waveInAddBuffer');

      _hdrs.add(hdr);
      _bufs.add(buf);
    }

    _check(_waveInStart(_hWaveIn), 'waveInStart');
    _active = true;
  }

  /// Collect any completed capture buffers. Returns empty if nothing ready.
  Uint8List poll() {
    if (!_active) return Uint8List(0);

    final out = BytesBuilder(copy: false);

    for (int i = 0; i < _numBufs; i++) {
      if (_hdrs[i].ref.dwFlags & _whdrDone != 0) {
        final n = _hdrs[i].ref.dwBytesRecorded;
        if (n > 0) {
          // Copy from native memory to Dart
          final chunk = Uint8List(n);
          final src = _bufs[i];
          for (int j = 0; j < n; j++) {
            chunk[j] = src[j];
          }
          out.add(chunk);
        }
        // Re-queue buffer
        _hdrs[i].ref.dwBytesRecorded = 0;
        _waveInAddBuffer(_hWaveIn, _hdrs[i], sizeOf<WAVEHDR>());
      }
    }

    return out.toBytes();
  }

  /// Stop recording and release resources.
  void stop() {
    if (!_active) return;
    _active = false;

    _waveInReset(_hWaveIn);

    for (int i = 0; i < _numBufs; i++) {
      _waveInUnprepareHeader(_hWaveIn, _hdrs[i], sizeOf<WAVEHDR>());
      calloc.free(_hdrs[i]);
      calloc.free(_bufs[i]);
    }
    _hdrs.clear();
    _bufs.clear();

    _waveInClose(_hWaveIn);
    _hWaveIn = 0;
  }

  void dispose() {
    stop();
    calloc.free(_hPtr);
    calloc.free(_fmt);
  }
}

// ══════════════════════════════════════════════════════════════════════
//  WinAudioPlayback — speaker output via waveOut
// ══════════════════════════════════════════════════════════════════════

class WinAudioPlayback {
  static const int _numBufs = 16;
  static const int _bufBytes = 640;

  int _hWaveOut = 0;
  bool _active = false;
  final Pointer<IntPtr> _hPtr = calloc<IntPtr>();
  final Pointer<WAVEFORMATEX> _fmt = _makeFormat();
  final List<Pointer<WAVEHDR>> _hdrs = [];
  final List<Pointer<Uint8>> _bufs = [];
  final List<bool> _busy = [];

  bool get isActive => _active;

  /// Open the default playback device.
  void start() {
    if (_active) return;

    final r = _waveOutOpen(_hPtr, _waveMapper, _fmt, 0, 0, _callbackNull);
    if (r != _mmsyserrNoerror) {
      throw AudioException('waveOutOpen failed (code $r). No audio output?');
    }
    _hWaveOut = _hPtr.value;

    for (int i = 0; i < _numBufs; i++) {
      final buf = calloc<Uint8>(_bufBytes);
      final hdr = calloc<WAVEHDR>();
      hdr.ref.lpData = buf;
      hdr.ref.dwBufferLength = _bufBytes;
      hdr.ref.dwFlags = 0;
      hdr.ref.dwLoops = 0;

      _check(_waveOutPrepareHeader(_hWaveOut, hdr, sizeOf<WAVEHDR>()),
          'waveOutPrepareHeader');

      _hdrs.add(hdr);
      _bufs.add(buf);
      _busy.add(false);
    }

    _active = true;
  }

  /// Feed PCM audio for playback. Returns true if accepted, false if all
  /// buffers are busy (audio will be dropped).
  bool feed(Uint8List data) {
    if (!_active || data.isEmpty) return false;

    // Reclaim completed buffers
    for (int i = 0; i < _numBufs; i++) {
      if (_busy[i] && (_hdrs[i].ref.dwFlags & _whdrDone) != 0) {
        _busy[i] = false;
      }
    }

    // Find a free buffer
    for (int i = 0; i < _numBufs; i++) {
      if (!_busy[i]) {
        final copyLen = data.length < _bufBytes ? data.length : _bufBytes;
        final dst = _bufs[i];
        for (int j = 0; j < copyLen; j++) {
          dst[j] = data[j];
        }
        _hdrs[i].ref.dwBufferLength = copyLen;

        final r = _waveOutWrite(_hWaveOut, _hdrs[i], sizeOf<WAVEHDR>());
        if (r == _mmsyserrNoerror) {
          _busy[i] = true;
          return true;
        }
        return false;
      }
    }

    return false; // all buffers busy — dropping frame
  }

  /// Stop playback and release resources.
  void stop() {
    if (!_active) return;
    _active = false;

    _waveOutReset(_hWaveOut);

    for (int i = 0; i < _numBufs; i++) {
      _waveOutUnprepareHeader(_hWaveOut, _hdrs[i], sizeOf<WAVEHDR>());
      calloc.free(_hdrs[i]);
      calloc.free(_bufs[i]);
    }
    _hdrs.clear();
    _bufs.clear();
    _busy.clear();

    _waveOutClose(_hWaveOut);
    _hWaveOut = 0;
  }

  void dispose() {
    stop();
    calloc.free(_hPtr);
    calloc.free(_fmt);
  }
}

// ── Helpers ──

void _check(int result, String fn) {
  if (result != _mmsyserrNoerror) {
    throw AudioException('$fn failed (code $result)');
  }
}

class AudioException implements Exception {
  final String message;
  AudioException(this.message);
  @override
  String toString() => 'AudioException: $message';
}
