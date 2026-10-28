//! Helpers shared by unit tests.

/// Minimal valid 8-bit mono PCM WAV, `ms` milliseconds of silence.
pub fn wav_bytes(ms: u32) -> Vec<u8> {
    let rate = 8000u32;
    let data_len = rate * ms / 1000;
    let mut v = Vec::new();
    v.extend(b"RIFF");
    v.extend((36 + data_len).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes()); // PCM
    v.extend(1u16.to_le_bytes()); // mono
    v.extend(rate.to_le_bytes());
    v.extend(rate.to_le_bytes()); // byte rate
    v.extend(1u16.to_le_bytes()); // block align
    v.extend(8u16.to_le_bytes()); // bits
    v.extend(b"data");
    v.extend(data_len.to_le_bytes());
    v.extend(std::iter::repeat_n(128u8, data_len as usize));
    v
}
