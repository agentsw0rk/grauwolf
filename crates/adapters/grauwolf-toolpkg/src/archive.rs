//! The source layer of a package: a ustar archive in a gzip stream, written by hand so that the
//! bytes are reproducible (sorted entries, fixed mtime/uid/gid, stored deflate blocks) and no
//! compression library is needed.

use std::collections::BTreeMap;

use crate::PackageFetchError;

const BLOCK: usize = 512;
const MAX_STORED: usize = 65_535;

/// The `.tar.gz` of `files` (path inside the archive → content). The same files always give the
/// same bytes.
pub fn source_archive(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, PackageFetchError> {
    let mut tar = Vec::new();
    for (name, content) in files {
        tar.extend_from_slice(&tar_header(name, content.len())?);
        tar.extend_from_slice(content);
        tar.resize(tar.len().div_ceil(BLOCK) * BLOCK, 0);
    }
    tar.extend_from_slice(&[0; 2 * BLOCK]);
    Ok(gzip_stored(&tar))
}

fn tar_header(name: &str, size: usize) -> Result<[u8; BLOCK], PackageFetchError> {
    if name.is_empty() || name.len() > 100 || !name.is_ascii() {
        return Err(PackageFetchError::InvalidPackage(format!(
            "source file name '{name}' must be 1 to 100 ASCII characters"
        )));
    }
    let mut header = [0u8; BLOCK];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    header[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].copy_from_slice(b"        ");
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    Ok(header)
}

fn gzip_stored(data: &[u8]) -> Vec<u8> {
    // Header: magic, deflate, no flags, mtime 0, no extra flags, OS unknown.
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    let mut chunks: Vec<&[u8]> = data.chunks(MAX_STORED).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let last = chunks.len() - 1;
    for (index, chunk) in chunks.into_iter().enumerate() {
        out.push(u8::from(index == last));
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }
    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads the stored blocks of a gzip stream and checks CRC and length.
    fn gunzip(gz: &[u8]) -> Vec<u8> {
        assert_eq!(&gz[..4], &[0x1f, 0x8b, 8, 0]);
        let mut pos = 10;
        let mut out = Vec::new();
        loop {
            let last = gz[pos] & 1 == 1;
            assert_eq!(gz[pos] >> 1, 0, "stored block expected");
            let len = u16::from_le_bytes([gz[pos + 1], gz[pos + 2]]) as usize;
            out.extend_from_slice(&gz[pos + 5..pos + 5 + len]);
            pos += 5 + len;
            if last {
                break;
            }
        }
        assert_eq!(
            u32::from_le_bytes(gz[pos..pos + 4].try_into().unwrap()),
            crc32(&out)
        );
        assert_eq!(
            u32::from_le_bytes(gz[pos + 4..pos + 8].try_into().unwrap()) as usize,
            out.len()
        );
        out
    }

    fn files(entries: &[(&str, &[u8])]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(name, content)| ((*name).to_owned(), content.to_vec()))
            .collect()
    }

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn the_archive_holds_the_files_sorted_with_fixed_metadata() {
        let tar =
            gunzip(&source_archive(&files(&[("src/b.rs", b"bbb"), ("src/a.rs", b"a")])).unwrap());
        assert_eq!(tar.len(), 2 * 1024 + 2 * 512);
        assert_eq!(&tar[..9], b"src/a.rs\0");
        assert_eq!(&tar[512..513], b"a");
        assert_eq!(&tar[1024..1033], b"src/b.rs\0");
        assert_eq!(&tar[1536..1539], b"bbb");
        // size, mtime, magic
        assert_eq!(&tar[124..136], b"00000000001\0");
        assert_eq!(&tar[136..148], b"00000000000\0");
        assert_eq!(&tar[257..263], b"ustar\0");
        // checksum of the first header is valid
        let mut header = tar[..512].to_vec();
        let stored =
            u32::from_str_radix(std::str::from_utf8(&header[148..154]).unwrap(), 8).unwrap();
        header[148..156].copy_from_slice(b"        ");
        assert_eq!(header.iter().map(|b| u32::from(*b)).sum::<u32>(), stored);
    }

    #[test]
    fn the_same_files_give_the_same_bytes() {
        let a = source_archive(&files(&[("x", b"1"), ("y", b"2")])).unwrap();
        let b = source_archive(&files(&[("y", b"2"), ("x", b"1")])).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn large_files_span_several_stored_blocks() {
        let big = vec![7u8; 150_000];
        let tar = gunzip(&source_archive(&files(&[("big", &big)])).unwrap());
        assert_eq!(&tar[512..512 + 150_000], &big[..]);
    }

    #[test]
    fn a_name_that_does_not_fit_is_refused() {
        let error = source_archive(&files(&[(&"n".repeat(101), b"")])).unwrap_err();
        assert!(error.to_string().contains("100"), "{error}");
    }
}
