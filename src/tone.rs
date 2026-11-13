//! The three chimes, synthesised (no sound files shipped). The same two pitches throughout, so
//! they read as one gesture: the press ascends, the save resolves (both together), a failure is
//! the press played backwards.

/// A note: start, frequency, amplitude, length, and the level of its second harmonic.
#[derive(Debug, Clone, Copy)]
pub struct Note {
    pub start_ms: u32,
    pub freq: f64,
    pub amp: f64,
    pub dur_ms: u32,
    pub harm: f64,
}

const fn note(start_ms: u32, freq: f64, amp: f64, dur_ms: u32, harm: f64) -> Note {
    Note {
        start_ms,
        freq,
        amp,
        dur_ms,
        harm,
    }
}

pub const SAVE: [Note; 2] = [note(0, 220.0, 0.34, 200, 0.5), note(0, 330.0, 0.18, 200, 0.0)];
pub const START: [Note; 2] = [note(0, 220.0, 0.20, 60, 0.0), note(55, 330.0, 0.20, 80, 0.0)];
pub const FAIL: [Note; 2] = [note(0, 330.0, 0.20, 60, 0.0), note(55, 220.0, 0.22, 110, 0.0)];

const RATE: u32 = 48000;
/// Without an attack the sine starts at full amplitude and clicks.
const ATTACK_S: f64 = 0.004;

/// A mono 16-bit WAV file, as PlaySoundW(SND_MEMORY) takes it. Its length comes from the notes:
/// a separate total once cut the last note mid-oscillation.
pub fn wav(notes: &[Note]) -> Vec<u8> {
    let rate = RATE as f64;
    let end_ms = notes.iter().map(|n| n.start_ms + n.dur_ms).max().unwrap_or(0);
    let mut mix = vec![0.0f64; (rate * end_ms as f64 / 1000.0) as usize];
    let attack = ((ATTACK_S * rate) as usize).max(1);
    for n in notes {
        let start = (rate * n.start_ms as f64 / 1000.0) as usize;
        let len = (rate * n.dur_ms as f64 / 1000.0) as usize;
        for i in 0..len.min(mix.len().saturating_sub(start)) {
            let t = i as f64 / rate;
            let envelope = (i as f64 / attack as f64).min(1.0) * (-5.0 * i as f64 / len as f64).exp();
            let wave =
                (std::f64::consts::TAU * n.freq * t).sin() + n.harm * (2.0 * std::f64::consts::TAU * n.freq * t).sin();
            mix[start + i] += n.amp * envelope * wave;
        }
    }
    let data: Vec<u8> = mix
        .iter()
        .flat_map(|v| ((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes())
        .collect();
    let mut out = Vec::with_capacity(44 + data.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // PCM header size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(wav: &[u8]) -> Vec<i16> {
        wav[44..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| i16::from_le_bytes(*b))
            .collect()
    }

    /// Same two pitches throughout, so the chimes read as one gesture: the press ascends, the
    /// save sounds both at once, a failure is the press played backwards.
    #[test]
    fn the_failure_is_the_press_played_backwards() {
        let pitches = |notes: &[Note]| notes.iter().map(|n| n.freq).collect::<Vec<_>>();
        let mut reversed = pitches(&START);
        reversed.reverse();
        assert_eq!(pitches(&FAIL), reversed);
        assert!(START[0].freq < START[1].freq, "the press ascends");
        assert!(
            SAVE.iter().all(|n| n.start_ms == 0),
            "the save sounds both notes together"
        );
    }

    #[test]
    fn a_valid_wav_as_long_as_its_last_note() {
        let w = wav(&FAIL);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(samples(&w).len(), (RATE as f64 * 0.165) as usize, "55 + 110 ms");
    }

    #[test]
    fn it_starts_silent_and_rises_instead_of_clicking() {
        let s = samples(&wav(&SAVE));
        assert_eq!(s[0], 0);
        assert!(s[..10].iter().all(|v| v.abs() < 800), "{:?}", &s[..10]);
        assert!(s.iter().any(|v| v.abs() > 5000), "inaudible");
    }
}
