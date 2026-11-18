//! Sound decoding and playback.
//!
//! Why this design: WinMM `PlaySound` is already the lightest way to play audio on Windows (no audio thread, no
//! decoder service, nothing kept in RAM), but it only understands WAV and has no volume control. So a sound is
//! decoded on demand with Symphonia (pure Rust; only the MP3, Ogg Vorbis and WAV codecs are compiled in),
//! scaled by the volume, wrapped into a tiny in-memory WAV and handed to `PlaySound`. Nothing is cached: the
//! decoded samples live only while the short-lived playback thread runs, so idle memory is unchanged.

use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, errors::Error as SymphoniaError,
    formats::FormatOptions, io::MediaSourceStream, meta::MetadataOptions, probe::Hint,
};

/// Source files above this size are rejected before any decoding.
pub const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;
/// Longer sounds are rejected: a notification chime is not a song, and this bounds decoded memory.
pub const MAX_SECONDS: u32 = 30;
pub const ALLOWED_EXTENSIONS: [&str; 3] = ["wav", "mp3", "ogg"];

pub struct Pcm {
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<i16>,
}

impl Pcm {
    pub fn seconds(&self) -> f32 {
        if self.channels == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        self.samples.len() as f32 / f32::from(self.channels) / self.sample_rate as f32
    }
}

/// Lower-case extension if it is one of the supported audio types.
pub fn allowed_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    ALLOWED_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Decodes WAV / MP3 / Ogg Vorbis bytes into 16-bit PCM. Errors are short codes (`E_SOUND_*`).
pub fn decode_bytes(bytes: Vec<u8>, extension: &str) -> Result<Pcm, String> {
    let stream = MediaSourceStream::new(Box::new(Cursor::new(bytes)), Default::default());
    let mut hint = Hint::new();
    hint.with_extension(extension);
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| "E_SOUND_DECODE".to_string())?;
    let mut format = probed.format;
    let track = format.default_track().ok_or("E_SOUND_DECODE")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| "E_SOUND_DECODE".to_string())?;

    let mut pcm = Pcm {
        channels: 0,
        sample_rate: 0,
        samples: Vec::new(),
    };
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(_)) | Err(SymphoniaError::ResetRequired) => break,
            Err(_) => return Err("E_SOUND_DECODE".into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue, // a damaged frame is skipped, not fatal
            Err(_) => return Err("E_SOUND_DECODE".into()),
        };
        let spec = *decoded.spec();
        pcm.channels = spec.channels.count() as u16;
        pcm.sample_rate = spec.rate;
        let mut buffer = SampleBuffer::<i16>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        pcm.samples.extend_from_slice(buffer.samples());
        let limit = (MAX_SECONDS as usize) * pcm.sample_rate as usize * pcm.channels as usize;
        if pcm.samples.len() > limit {
            return Err("E_SOUND_TOO_LONG".into());
        }
    }
    if pcm.channels == 0 || pcm.channels > 2 || pcm.sample_rate == 0 || pcm.samples.is_empty() {
        return Err("E_SOUND_DECODE".into());
    }
    Ok(pcm)
}

pub fn decode_file(path: &Path) -> Result<Pcm, String> {
    let extension = allowed_extension(path).ok_or("E_SOUND_INVALID")?;
    let meta = fs::metadata(path).map_err(|_| "E_SOUND_MISSING".to_string())?;
    if !meta.is_file() {
        return Err("E_SOUND_MISSING".into());
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err("E_SOUND_TOO_LARGE".into());
    }
    decode_bytes(
        fs::read(path).map_err(|_| "E_SOUND_MISSING".to_string())?,
        &extension,
    )
}

/// Perceptual volume curve: 0..=100 maps to a gain of 0.0..=1.0 with finer control at the quiet end.
pub fn gain_from_volume(volume: u8) -> f32 {
    let v = f32::from(volume.min(100)) / 100.0;
    v * v
}

/// 16-bit PCM RIFF/WAVE image with the gain applied (clamped, never wraps).
pub fn to_wav(pcm: &Pcm, gain: f32) -> Vec<u8> {
    let data_len = (pcm.samples.len() * 2) as u32;
    let block_align = pcm.channels * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&pcm.channels.to_le_bytes());
    out.extend_from_slice(&pcm.sample_rate.to_le_bytes());
    out.extend_from_slice(&(pcm.sample_rate * u32::from(block_align)).to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm.samples {
        let scaled = (f32::from(*sample) * gain).round().clamp(-32768.0, 32767.0) as i16;
        out.extend_from_slice(&scaled.to_le_bytes());
    }
    out
}

/// Where a notification sound comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// The bundled chime (embedded in the executable).
    Default,
    /// A copy inside the app's own data folder.
    File(PathBuf),
}

/// Resolves a source to PCM. A missing or broken custom file falls back to the bundled chime so a moved or
/// deleted file can never silence or crash a notification; the second value says whether the fallback was used.
pub fn load(source: &Source, default_wav: &[u8]) -> (Result<Pcm, String>, bool) {
    match source {
        Source::Default => (decode_bytes(default_wav.to_vec(), "wav"), false),
        Source::File(path) => match decode_file(path) {
            Ok(pcm) => (Ok(pcm), false),
            Err(_) => (decode_bytes(default_wav.to_vec(), "wav"), true),
        },
    }
}

/// Plays a prepared WAV image and blocks until it ends (or until another sound interrupts it).
/// Call from a short-lived thread only.
#[cfg(windows)]
pub fn play_wav_blocking(wav: &[u8]) -> Result<(), String> {
    use std::ffi::c_void;
    #[link(name = "winmm")]
    unsafe extern "system" {
        fn PlaySoundW(sound: *const u16, hmod: *mut c_void, flags: u32) -> i32;
    }
    const SND_SYNC: u32 = 0x0000;
    const SND_NODEFAULT: u32 = 0x0002;
    const SND_MEMORY: u32 = 0x0004;
    // SAFETY: `wav` outlives the call because the call is synchronous; PlaySound only reads the buffer.
    let ok = unsafe {
        PlaySoundW(
            wav.as_ptr().cast(),
            std::ptr::null_mut(),
            SND_SYNC | SND_NODEFAULT | SND_MEMORY,
        )
    };
    if ok == 0 {
        Err("E_SOUND_PLAY".into())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn play_wav_blocking(_wav: &[u8]) -> Result<(), String> {
    Err("E_UNSUPPORTED_OS".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAV: &[u8] = include_bytes!("../test-fixtures/tone.wav");
    const MP3: &[u8] = include_bytes!("../test-fixtures/tone.mp3");
    const OGG: &[u8] = include_bytes!("../test-fixtures/tone.ogg");

    #[test]
    fn decodes_wav_mp3_and_ogg() {
        for (bytes, ext, channels) in [(WAV, "wav", 2), (MP3, "mp3", 1), (OGG, "ogg", 1)] {
            let pcm = decode_bytes(bytes.to_vec(), ext).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(pcm.channels, channels, "{ext}");
            assert!(
                (0.2..0.6).contains(&pcm.seconds()),
                "{ext}: {}",
                pcm.seconds()
            );
            assert!(pcm.samples.iter().any(|s| s.abs() > 1000), "{ext}: audible");
        }
    }

    #[test]
    fn garbage_is_rejected_not_panicked() {
        assert_eq!(
            decode_bytes(vec![1, 2, 3, 4, 5], "wav").err().as_deref(),
            Some("E_SOUND_DECODE")
        );
        assert!(decode_bytes(vec![], "mp3").is_err());
        assert!(decode_bytes(b"RIFFxxxxWAVEfmt ".to_vec(), "wav").is_err());
    }

    #[test]
    fn volume_curve_is_monotonic_and_bounded() {
        assert_eq!(gain_from_volume(0), 0.0);
        assert_eq!(gain_from_volume(100), 1.0);
        assert_eq!(gain_from_volume(200), 1.0, "clamped");
        let mut last = -1.0;
        for v in 0..=100 {
            let g = gain_from_volume(v);
            assert!(g >= last);
            last = g;
        }
        assert!(gain_from_volume(50) < 0.5, "quiet end has finer control");
    }

    #[test]
    #[allow(clippy::chunks_exact_to_as_chunks)]
    fn wav_image_is_valid_and_volume_scales_samples() {
        let pcm = decode_bytes(WAV.to_vec(), "wav").unwrap();
        let full = to_wav(&pcm, 1.0);
        let half = to_wav(&pcm, 0.5);
        assert_eq!(&full[0..4], b"RIFF");
        assert_eq!(&full[8..12], b"WAVE");
        assert_eq!(full.len(), 44 + pcm.samples.len() * 2);
        let again = decode_bytes(full.clone(), "wav").unwrap();
        assert_eq!(again.samples.len(), pcm.samples.len());
        let peak = |bytes: &[u8]| {
            bytes[44..]
                .chunks_exact(2)
                .map(|b| i16::from_le_bytes([b[0], b[1]]).unsigned_abs())
                .max()
                .unwrap()
        };
        let (p_full, p_half) = (peak(&full), peak(&half));
        assert!(
            p_half * 2 >= p_full - 2 && p_half * 2 <= p_full + 2,
            "{p_full} {p_half}"
        );
        assert_eq!(peak(&to_wav(&pcm, 0.0)), 0, "volume 0 is silent");
    }

    #[test]
    fn scaling_never_wraps_around() {
        let pcm = Pcm {
            channels: 1,
            sample_rate: 8000,
            samples: vec![i16::MAX, i16::MIN],
        };
        let wav = to_wav(&pcm, 4.0);
        assert_eq!(i16::from_le_bytes([wav[44], wav[45]]), i16::MAX);
        assert_eq!(i16::from_le_bytes([wav[46], wav[47]]), i16::MIN);
    }

    #[test]
    fn extension_allow_list() {
        assert_eq!(
            allowed_extension(Path::new("a/B.WAV")).as_deref(),
            Some("wav")
        );
        assert_eq!(
            allowed_extension(Path::new("x.Mp3")).as_deref(),
            Some("mp3")
        );
        assert_eq!(
            allowed_extension(Path::new("x.ogg")).as_deref(),
            Some("ogg")
        );
        for bad in ["x.exe", "x.ps1", "x.mp4", "x", "x.wav.exe", ".wav"] {
            assert_eq!(allowed_extension(Path::new(bad)), None, "{bad}");
        }
    }

    #[test]
    fn missing_custom_file_falls_back_to_default() {
        let (pcm, fell_back) = load(
            &Source::File(PathBuf::from("Z:/definitely/not/here.mp3")),
            WAV,
        );
        assert!(pcm.is_ok() && fell_back);
        let (pcm, fell_back) = load(&Source::Default, WAV);
        assert!(pcm.is_ok() && !fell_back);
    }

    #[test]
    fn oversized_and_non_audio_files_are_refused() {
        let dir = std::env::temp_dir().join(format!("kn-audio-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let text = dir.join("fake.wav");
        fs::write(&text, b"not audio at all").unwrap();
        assert_eq!(decode_file(&text).err().as_deref(), Some("E_SOUND_DECODE"));
        let exe = dir.join("run.exe");
        fs::write(&exe, WAV).unwrap();
        assert_eq!(decode_file(&exe).err().as_deref(), Some("E_SOUND_INVALID"));
        assert_eq!(
            decode_file(&dir.join("nope.wav")).err().as_deref(),
            Some("E_SOUND_MISSING")
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
