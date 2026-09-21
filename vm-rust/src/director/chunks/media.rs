use binary_reader::{BinaryReader, Endian};
use std::convert::TryInto;

use log::{debug};

fn mp3_frame_len(h: &[u8]) -> Option<usize> {
    if h.len() < 4 || h[0] != 0xFF || (h[1] & 0xE0) != 0xE0 {
        return None;
    }
    let version_id = (h[1] >> 3) & 0x03;
    let layer = (h[1] >> 1) & 0x03;
    let bitrate_idx = (h[2] >> 4) & 0x0F;
    let rate_idx = (h[2] >> 2) & 0x03;
    if version_id == 1 || layer == 0 || bitrate_idx == 0 || bitrate_idx == 15 || rate_idx == 3 {
        return None;
    }
    let mpeg1 = version_id == 3;
    const V1L1: [u32; 16] = [0,32,64,96,128,160,192,224,256,288,320,352,384,416,448,0];
    const V1L2: [u32; 16] = [0,32,48,56,64,80,96,112,128,160,192,224,256,320,384,0];
    const V1L3: [u32; 16] = [0,32,40,48,56,64,80,96,112,128,160,192,224,256,320,0];
    const V2L1: [u32; 16] = [0,32,48,56,64,80,96,112,128,144,160,176,192,224,256,0];
    const V2L23: [u32; 16] = [0,8,16,24,32,40,48,56,64,80,96,112,128,144,160,0];
    let bitrate = match (mpeg1, layer) {
        (true, 3) => V1L1[bitrate_idx as usize],
        (true, 2) => V1L2[bitrate_idx as usize],
        (true, 1) => V1L3[bitrate_idx as usize],
        (false, 3) => V2L1[bitrate_idx as usize],
        (false, _) => V2L23[bitrate_idx as usize],
        _ => 0,
    } * 1000;
    if bitrate == 0 {
        return None;
    }
    let base_rate = [44100u32, 48000, 32000][rate_idx as usize];
    let sample_rate = match version_id {
        3 => base_rate,
        2 => base_rate / 2,
        _ => base_rate / 4,
    };
    let padding = ((h[2] >> 1) & 0x01) as u32;
    let len = if layer == 3 {
        (12 * bitrate / sample_rate + padding) * 4
    } else {
        let coeff = if mpeg1 { 144 } else { 72 };
        coeff * bitrate / sample_rate + padding
    };
    (len >= 4).then_some(len as usize)
}

#[derive(Debug, Clone)]
pub struct MediaChunk {
    pub sample_rate: u32,
    pub data_size_field: u32,
    pub guid: Option<[u8; 16]>,
    pub audio_data: Vec<u8>,
    pub is_compressed: bool,
}

impl MediaChunk {
    /// Find a valid MPEG frame after a short Director media prefix. Require two
    /// complete chained frames so arbitrary PCM bytes are not classified as MP3.
    pub(crate) fn find_mp3_start(data: &[u8]) -> Option<usize> {
        let limit = data.len().min(2048);
        for off in 0..limit {
            let Some(first_len) = mp3_frame_len(&data[off..]) else {
                continue;
            };
            let first_end = off.checked_add(first_len)?;
            if first_end > data.len() {
                continue;
            }
            if first_end == data.len() {
                return Some(off);
            }
            let next = &data[first_end..];
            let Some(next_len) = mp3_frame_len(next) else {
                continue;
            };
            if next_len <= next.len() {
                return Some(off);
            }
        }
        None
    }

    fn has_explicit_ima_guid(&self) -> bool {
        self.guid.is_some_and(|guid| {
            &guid[0..8] == &[0x5A, 0x08, 0xCD, 0x40, 0x53, 0x5B, 0x11, 0xD0]
        })
    }

    pub(crate) fn mp3_start(&self) -> Option<usize> {
        (!self.has_explicit_ima_guid())
            .then(|| Self::find_mp3_start(&self.audio_data))
            .flatten()
    }

    pub fn from_reader(reader: &mut BinaryReader) -> Result<Self, String> {
        let mut data_test = Vec::new();

        let r_begin = reader.pos;
        while let Ok(byte) = reader.read_u8() {
            data_test.push(byte);
        }

        let hex_dump = data_test
            .iter()
            .map(|b| format!("{:02X} ", b))
            .collect::<Vec<String>>()
            .join(" ");

        debug!(
            "WAV Hex Dump (Full File, {} bytes):\n{}",
            data_test.len(),
            hex_dump
        );

        reader.pos = r_begin;

        // Detect JPEG data before parsing sound header.
        // MediaChunk is used for both sound data (with a sound header) and JPEG bitmap
        // data (no header). If we parse the sound header on JPEG data, the JPEG magic
        // bytes get consumed and the data is lost.
        if data_test.len() >= 3 && data_test[0] == 0xFF && data_test[1] == 0xD8 && data_test[2] == 0xFF {
            debug!("MediaChunk: detected JPEG data ({} bytes), skipping sound header parse", data_test.len());
            return Ok(MediaChunk {
                sample_rate: 0,
                data_size_field: data_test.len() as u32,
                guid: None,
                audio_data: data_test,
                is_compressed: false,
            });
        }

        // Detect an ID3v2-tagged MP3 before parsing the sound header, for the same
        // reason as the JPEG check above: there is no Director sound header here, and
        // parsing one reads the tag's own text as numeric fields.
        //
        // AreaZero's sound members are MP3s carrying an ID3v2 tag, so they begin with
        // "ID3" rather than an MP3 frame sync — the frame-sync test in get_codec_name
        // missed them, and the header parse produced identical garbage on every member
        // (headerSize 0x49443303 = "ID3\x03", sampleRate 0x07765443 = "\x07vTC",
        // dataSizeField 0x00426C75 = "\0Blu" — the TCON genre frame, "Blues"). The
        // bogus dataSizeField then tripped the compression-ratio heuristic, so they
        // were reported as ima_adpcm at 125195331 Hz.
        //
        // Strip the tag so the data starts at a real frame sync and the normal MP3
        // path takes over; the decoder reads the true sample rate from the stream.
        if data_test.len() >= 10 && &data_test[0..3] == b"ID3" {
            let flags = data_test[5];
            // ID3v2 sizes are synchsafe: 7 bits per byte, high bit always clear.
            let tag_size = ((data_test[6] as usize & 0x7F) << 21)
                | ((data_test[7] as usize & 0x7F) << 14)
                | ((data_test[8] as usize & 0x7F) << 7)
                | (data_test[9] as usize & 0x7F);
            // 10-byte header, plus a 10-byte footer when the footer flag is set.
            let tag_len = 10 + tag_size + if flags & 0x10 != 0 { 10 } else { 0 };
            if tag_len < data_test.len() {
                let audio_data = data_test[tag_len..].to_vec();
                debug!(
                    "MediaChunk: ID3v2 tag of {} bytes stripped, {} bytes of MP3 remain",
                    tag_len,
                    audio_data.len()
                );
                return Ok(MediaChunk {
                    sample_rate: 0, // the MP3 stream carries its own rate
                    data_size_field: audio_data.len() as u32,
                    guid: None,
                    audio_data,
                    is_compressed: true,
                });
            }
            debug!(
                "MediaChunk: ID3v2 tag length {} >= data length {}; parsing as-is",
                tag_len,
                data_test.len()
            );
        }

        let original_endian = reader.endian;
        reader.endian = Endian::Big;

        let header_size = reader.read_u32().map_err(|e| e.to_string())?;
        let _unknown1 = reader.read_u32().map_err(|e| e.to_string())?;
        let sample_rate = reader.read_u32().map_err(|e| e.to_string())?;
        let _sample_rate2 = reader.read_u32().map_err(|e| e.to_string())?;
        let _unknown2 = reader.read_u32().map_err(|e| e.to_string())?;
        let data_size_field = reader.read_u32().map_err(|e| e.to_string())?;

        let bytes_read = 24;
        let skip_bytes = (header_size as usize).saturating_sub(bytes_read);

        // Read GUID if present
        let guid: Option<[u8; 16]> = if skip_bytes >= 16 {
            let b = reader.read_bytes(16).map_err(|e| e.to_string())?;
            Some(b.try_into().unwrap())
        } else {
            None
        };

        // Skip remaining header padding
        if skip_bytes > 16 {
            let _ = reader.read_bytes(skip_bytes - 16);
        } else if skip_bytes > 0 && skip_bytes < 16 {
            let _ = reader.read_bytes(skip_bytes);
        }

        // Read all remaining data as audio data
        let mut audio_data = Vec::new();
        while let Ok(byte) = reader.read_u8() {
            audio_data.push(byte);
        }

        // Director media can prepend a short framing field before MP3. Use the
        // chained-frame validator rather than an offset-zero sync heuristic.
        let is_mp3 = !guid.is_some_and(|guid| {
            &guid[0..8] == &[0x5A, 0x08, 0xCD, 0x40, 0x53, 0x5B, 0x11, 0xD0]
        }) && MediaChunk::find_mp3_start(&audio_data).is_some();

        // IMA ADPCM: data is significantly smaller than data_size_field
        // data_size_field represents uncompressed PCM size
        let compression_ratio = if audio_data.len() > 0 {
            data_size_field as f32 / audio_data.len() as f32
        } else {
            1.0
        };

        let is_ima_adpcm = compression_ratio > 2.0 && !is_mp3;
        let is_compressed = is_mp3 || is_ima_adpcm;

        debug!(
            "MediaChunk: {} bytes (expected {}), ratio={:.2}, mp3={}, ima_adpcm={}, rate={}",
            audio_data.len(),
            data_size_field,
            compression_ratio,
            is_mp3,
            is_ima_adpcm,
            sample_rate
        );

        reader.endian = original_endian;

        Ok(MediaChunk {
            sample_rate,
            data_size_field,
            guid,
            audio_data,
            is_compressed,
        })
    }

    // Helper to extract sample rate from MP3 frame header
    fn get_mp3_sample_rate(frame_header: &[u8]) -> Option<u32> {
        if frame_header.len() < 4 {
            return None;
        }

        // MP3 frame: FF Fx xx xx
        // Byte 2, bits 2-3 contain sample rate index
        let sample_rate_bits = (frame_header[2] >> 2) & 0x03;

        // MPEG version from byte 1, bits 3-4
        let mpeg_version = (frame_header[1] >> 3) & 0x03;

        match (mpeg_version, sample_rate_bits) {
            (3, 0) => Some(44100), // MPEG-1
            (3, 1) => Some(48000),
            (3, 2) => Some(32000),
            (2, 0) => Some(22050), // MPEG-2
            (2, 1) => Some(24000),
            (2, 2) => Some(16000),
            (0, 0) => Some(11025), // MPEG-2.5
            (0, 1) => Some(12000),
            (0, 2) => Some(8000),
            _ => None,
        }
    }

    pub fn get_codec_name(&self) -> &str {
        if self.has_explicit_ima_guid() {
            return "ima_adpcm";
        }

        if self.mp3_start().is_some() {
            return "mp3";
        }

        // Check for IMA ADPCM by compression ratio
        let compression_ratio = if self.audio_data.len() > 0 {
            self.data_size_field as f32 / self.audio_data.len() as f32
        } else {
            1.0
        };

        if compression_ratio > 2.0 {
            "ima_adpcm"
        } else {
            "raw_pcm"
        }
    }

    pub fn is_sound(&self) -> bool {
        // Consider both compressed (MP3) and raw PCM as valid sound
        self.is_compressed || !self.audio_data.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::MediaChunk;
    use sha2::{Digest, Sha256};

    const DCR_MEDIA: &[u8] = include_bytes!("../../../tests/fixtures/spybot_s_win_flag_dcr_media.bin");
    const DCR_MEDIA_SHA256: &str =
        "e274ab9e1d3f0d04d00cf5e27a37dec72f4cfb199e4cd8f2f158ee195df1c55f";

    #[test]
    fn dcr_media_prefix_is_trimmed_as_mp3() {
        let media = MediaChunk {
            sample_rate: 16_000,
            data_size_field: 22_198,
            guid: Some([
                0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
                0, 2, 0, 1, 77, 65, 67, 82,
            ]),
            audio_data: DCR_MEDIA.to_vec(),
            is_compressed: true,
        };
        assert_eq!(format!("{:x}", Sha256::digest(DCR_MEDIA)), DCR_MEDIA_SHA256);
        assert_eq!(MediaChunk::find_mp3_start(DCR_MEDIA), Some(4));
        assert_eq!(media.get_codec_name(), "mp3");

        let sound = crate::director::chunks::sound::SoundChunk::from_media(&media);
        assert_eq!(sound.codec(), "mp3");
        assert_eq!(sound.sample_rate(), 16_000);
        assert_eq!(sound.sample_count(), 0);
        assert_eq!(sound.data().len(), 3_528);
        assert_eq!(&sound.data()[..4], &[0xff, 0xf3, 0x28, 0xc4]);
    }

    #[test]
    fn mp3_prefix_classifier_rejects_false_sync_and_truncation() {
        assert_eq!(MediaChunk::find_mp3_start(&[0xff, 0xf3, 0x28, 0xc4]), None);
        assert_eq!(MediaChunk::find_mp3_start(&[0; 512]), None);
        let first_len = super::mp3_frame_len(&DCR_MEDIA[4..]).unwrap();
        assert_eq!(MediaChunk::find_mp3_start(&DCR_MEDIA[..4 + first_len + 1]), None);
    }

    #[test]
    fn explicit_ima_guid_takes_precedence_over_mp3_sync() {
        let media = MediaChunk {
            sample_rate: 16_000,
            data_size_field: 22_198,
            guid: Some([0x5a, 0x08, 0xcd, 0x40, 0x53, 0x5b, 0x11, 0xd0, 0, 0, 0, 0, 0, 0, 0, 0]),
            audio_data: DCR_MEDIA.to_vec(),
            is_compressed: true,
        };
        assert_eq!(media.get_codec_name(), "ima_adpcm");
        assert_eq!(media.mp3_start(), None);
    }
}
