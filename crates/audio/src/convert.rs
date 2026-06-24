//! Audio format conversion — decode any supported format to WAV.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use rodio::Decoder;

use crate::{AudioError, AudioResult};

/// Convert an audio file of any supported format to a 32-bit float WAV file.
///
/// Supported input formats: WAV, MP3, FLAC, OGG Vorbis, AAC/M4A.
/// The output is always a 32-bit float WAV at the source sample rate.
///
/// Returns the path to the output WAV file.
pub fn convert_to_wav(input: &Path, output: &Path) -> AudioResult<()> {
    let file = File::open(input)
        .map_err(|e| AudioError::Encoding(format!("Cannot open {}: {e}", input.display())))?;

    let reader = BufReader::new(file);

    let decoder = Decoder::new(reader)
        .map_err(|e| AudioError::Encoding(format!("Cannot decode {}: {e}", input.display())))?;

    let sample_rate = decoder.sample_rate();
    let channels = decoder.channels();

    // Collect all samples as f32.
    let samples: Vec<f32> = decoder.convert_samples::<f32>().collect();

    if samples.is_empty() {
        return Err(AudioError::Encoding("Decoded audio is empty".into()));
    }

    // Write as WAV.
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let mut writer = hound::WavWriter::create(output, spec)
        .map_err(|e| AudioError::Encoding(format!("Cannot create WAV: {e}")))?;

    for sample in &samples {
        writer
            .write_sample(*sample)
            .map_err(|e| AudioError::Encoding(format!("WAV write error: {e}")))?;
    }

    writer
        .finalize()
        .map_err(|e| AudioError::Encoding(format!("WAV finalize error: {e}")))?;

    Ok(())
}

/// Returns true if the file has a WAV extension.
pub fn is_wav_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.eq_ignore_ascii_case("wav"))
        .unwrap_or(false)
}

use rodio::Source;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_wav_detection() {
        assert!(is_wav_file(Path::new("audio.wav")));
        assert!(is_wav_file(Path::new("audio.WAV")));
        assert!(!is_wav_file(Path::new("audio.mp3")));
        assert!(!is_wav_file(Path::new("audio.flac")));
        assert!(!is_wav_file(Path::new("noext")));
    }
}
