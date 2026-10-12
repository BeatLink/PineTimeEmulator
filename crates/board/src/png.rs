// SPDX-License-Identifier: GPL-3.0-only
//! A small PNG writer for 8-bit RGB images, storing the pixels uncompressed inside zlib.

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];
const LARGEST_STORED_BLOCK: usize = 65_535;

// ============================================================================
// Encoding
// ============================================================================

/// Encodes `rgb`, rows of `width` packed RGB pixels, as a PNG file.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    let row_bytes = width as usize * 3;
    assert_eq!(
        rgb.len(),
        row_bytes * height as usize,
        "the pixels do not fill the image"
    );

    let mut rows = Vec::with_capacity((row_bytes + 1) * height as usize);
    for row in rgb.chunks(row_bytes.max(1)).take(height as usize) {
        rows.push(0);
        rows.extend_from_slice(row);
    }

    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&width.to_be_bytes());
    header.extend_from_slice(&height.to_be_bytes());
    header.extend_from_slice(&[8, 2, 0, 0, 0]);

    let mut file = Vec::with_capacity(rows.len() + rows.len() / LARGEST_STORED_BLOCK * 5 + 64);
    file.extend_from_slice(&SIGNATURE);
    write_chunk(&mut file, b"IHDR", &header);
    write_chunk(&mut file, b"IDAT", &zlib_stored(&rows));
    write_chunk(&mut file, b"IEND", &[]);
    file
}

fn write_chunk(file: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    file.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let body_start = file.len();
    file.extend_from_slice(kind);
    file.extend_from_slice(data);
    let checksum = crc32(&file[body_start..]);
    file.extend_from_slice(&checksum.to_be_bytes());
}

/// Wraps `data` in a zlib stream of stored deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    let mut blocks = data.chunks(LARGEST_STORED_BLOCK).peekable();
    if blocks.peek().is_none() {
        stream.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(block) = blocks.next() {
        let last = blocks.peek().is_none();
        let length = block.len() as u16;
        stream.push(last as u8);
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(block);
    }
    stream.extend_from_slice(&adler32(data).to_be_bytes());
    stream
}

// ============================================================================
// Checksums
// ============================================================================

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MODULUS: u32 = 65_521;
    let (mut low, mut high) = (1u32, 0u32);
    for chunk in data.chunks(5_552) {
        for byte in chunk {
            low += *byte as u32;
            high += low;
        }
        low %= MODULUS;
        high %= MODULUS;
    }
    (high << 16) | low
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksums_match_known_values() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn writes_a_signature_header_and_end_chunk() {
        let file = encode_rgb(2, 1, &[255, 0, 0, 0, 255, 0]);

        assert_eq!(&file[..8], &SIGNATURE);
        assert_eq!(&file[12..16], b"IHDR");
        assert_eq!(&file[16..24], &[0, 0, 0, 2, 0, 0, 0, 1]);
        assert_eq!(&file[file.len() - 8..file.len() - 4], b"IEND");
    }

    #[test]
    fn stores_rows_with_a_filter_byte_each() {
        let file = encode_rgb(1, 2, &[1, 2, 3, 4, 5, 6]);
        let idat = file.windows(4).position(|window| window == b"IDAT").unwrap();
        let stream = &file[idat + 4..];

        assert_eq!(&stream[..2], &[0x78, 0x01]);
        assert_eq!(stream[2], 1);
        assert_eq!(&stream[3..5], &8u16.to_le_bytes());
        assert_eq!(&stream[7..15], &[0, 1, 2, 3, 0, 4, 5, 6]);
    }

    #[test]
    fn splits_large_images_into_several_stored_blocks() {
        let stream = zlib_stored(&vec![7; LARGEST_STORED_BLOCK + 10]);

        assert_eq!(stream[2], 0);
        let second_block = 2 + 5 + LARGEST_STORED_BLOCK;
        assert_eq!(stream[second_block], 1);
        assert_eq!(&stream[second_block + 1..second_block + 3], &10u16.to_le_bytes());
    }
}
