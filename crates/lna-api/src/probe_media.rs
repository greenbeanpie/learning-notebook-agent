//! Small media samples for "test connection": image / audio / video input is detected by sending
//! one sample of each kind and checking the endpoint answers about its content.

use base64::Engine;

/// The colour painted into the image and video samples, and the words accepted for it.
pub const SAMPLE_COLOR_WORDS: &[&str] = &["red", "红"];

/// A 64×64 solid red PNG.
pub fn red_png() -> Vec<u8> {
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb([230, 20, 20]));
    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut buf, image::ImageFormat::Png)
        .expect("encoding a PNG into memory cannot fail");
    buf.into_inner()
}

/// A two-second 64×64 solid red MP4 (MPEG-4 Part 2), 1 KB.
pub const RED_MP4: &[u8] = include_bytes!("../assets/probe/red.mp4");

/// How many separate beeps the audio sample holds. A model has to count them, so a text-only
/// model that explains it received no audio cannot pass by guessing.
pub const BEEP_COUNT: usize = 5;

/// Five 200 ms beeps (440 Hz) separated by 200 ms of silence: 16 kHz, 16-bit mono PCM WAV.
pub fn tone_wav() -> Vec<u8> {
    const RATE: u32 = 16_000;
    const SEGMENT: u32 = RATE / 5; // 200 ms
    let samples: Vec<i16> = (0..(BEEP_COUNT as u32 * 2 - 1) * SEGMENT)
        .map(|i| {
            if (i / SEGMENT) % 2 == 1 {
                return 0;
            }
            let t = i as f32 / RATE as f32;
            ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
        })
        .collect();
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The marker a model is asked to reply with when it got no audio.
pub const NO_AUDIO: &str = "NO_AUDIO";

/// Whether a reply to the colour question names the sample colour.
pub fn names_sample_color(reply: &str) -> bool {
    let r = reply.to_lowercase();
    SAMPLE_COLOR_WORDS.iter().any(|w| r.contains(w))
}

/// Whether a reply to the audio question shows the audio arrived: it counts the beeps. A reply
/// that merely says something about an attachment (e.g. "I received only text") does not pass.
pub fn heard_audio(reply: &str) -> bool {
    let r = reply.trim().to_lowercase();
    if r.is_empty() || r.contains(&NO_AUDIO.to_lowercase()) {
        return false;
    }
    let digits: Vec<String> = r
        .split(|c: char| !c.is_ascii_digit())
        .filter(|d| !d.is_empty())
        .map(str::to_string)
        .collect();
    digits.len() == 1 && digits[0] == BEEP_COUNT.to_string()
        || digits.is_empty() && (r.contains("five") || r.contains('五'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_well_formed() {
        let png = red_png();
        assert_eq!(&png[1..4], b"PNG");
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!(img.get_pixel(10, 10).0, [230, 20, 20]);
        let wav = tone_wav();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 2 * 9 * 3_200);
        assert_eq!(&RED_MP4[4..8], b"ftyp");
    }

    #[test]
    fn reply_checks() {
        assert!(names_sample_color("Red."));
        assert!(names_sample_color("红色"));
        assert!(!names_sample_color("I can't see any image."));
        assert!(heard_audio("5"));
        assert!(heard_audio("Five beeps."));
        assert!(heard_audio("五声"));
        assert!(!heard_audio("NO_AUDIO"));
        assert!(!heard_audio("  "));
        assert!(!heard_audio(
            "I received only text in this conversation. I do not see any attachment."
        ));
        assert!(!heard_audio("3"));
        assert!(!heard_audio("I count 3 or 5 beeps"));
    }
}
