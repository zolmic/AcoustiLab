//! 32-bit IEEE-float WAV (spec Section 14: impulse responses export as
//! 32-bit float WAV at 48 or 96 kHz).
//!
//! Layout (RIFF, little-endian): `RIFF` size `WAVE`, a `fmt ` chunk of 18
//! bytes (format tag 3 = WAVE_FORMAT_IEEE_FLOAT, channels, sample rate,
//! byte rate, block align, 32 bits, cbSize 0), the `fact` chunk that
//! non-PCM formats require (frame count), and `data` with interleaved
//! samples. Values are written as they are, without normalisation or
//! clipping; float WAV holds any finite value.

/// Encodes channels of equal length as a float WAV file.
pub fn float_wav(channels: &[&[f64]], sample_rate: u32) -> Result<Vec<u8>, String> {
    let nch = channels.len();
    if nch == 0 || nch > 16 {
        return Err("WAV: 1 to 16 channels".into());
    }
    let frames = channels[0].len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err("WAV: channels differ in length".into());
    }
    if channels
        .iter()
        .flat_map(|c| c.iter())
        .any(|v| !v.is_finite())
    {
        return Err("WAV: non-finite sample".into());
    }
    let data_len = frames
        .checked_mul(nch * 4)
        .filter(|&d| d <= u32::MAX as usize - 64)
        .ok_or("WAV: too long")?;
    let mut out = Vec::with_capacity(58 + data_len);
    let u16le = |out: &mut Vec<u8>, v: u16| out.extend_from_slice(&v.to_le_bytes());
    let u32le = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
    out.extend_from_slice(b"RIFF");
    u32le(&mut out, (4 + (8 + 18) + (8 + 4) + 8 + data_len) as u32);
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    u32le(&mut out, 18);
    u16le(&mut out, 3);
    u16le(&mut out, nch as u16);
    u32le(&mut out, sample_rate);
    u32le(&mut out, sample_rate * 4 * nch as u32);
    u16le(&mut out, (4 * nch) as u16);
    u16le(&mut out, 32);
    u16le(&mut out, 0);
    out.extend_from_slice(b"fact");
    u32le(&mut out, 4);
    u32le(&mut out, frames as u32);
    out.extend_from_slice(b"data");
    u32le(&mut out, data_len as u32);
    for i in 0..frames {
        for c in channels {
            out.extend_from_slice(&(c[i] as f32).to_le_bytes());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_layout() {
        let w = float_wav(&[&[0.5, -0.25]], 48_000).unwrap();
        assert_eq!(w.len(), 58 + 8);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()), 58);
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[16..20].try_into().unwrap()), 18);
        assert_eq!(u16::from_le_bytes(w[20..22].try_into().unwrap()), 3);
        assert_eq!(u16::from_le_bytes(w[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 48_000);
        assert_eq!(u32::from_le_bytes(w[28..32].try_into().unwrap()), 192_000);
        assert_eq!(u16::from_le_bytes(w[32..34].try_into().unwrap()), 4);
        assert_eq!(u16::from_le_bytes(w[34..36].try_into().unwrap()), 32);
        assert_eq!(&w[38..42], b"fact");
        assert_eq!(u32::from_le_bytes(w[46..50].try_into().unwrap()), 2);
        assert_eq!(&w[50..54], b"data");
        assert_eq!(u32::from_le_bytes(w[54..58].try_into().unwrap()), 8);
        assert_eq!(f32::from_le_bytes(w[58..62].try_into().unwrap()), 0.5);
        assert_eq!(f32::from_le_bytes(w[62..66].try_into().unwrap()), -0.25);
    }
}
