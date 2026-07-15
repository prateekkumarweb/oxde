use std::{
    io::{self, Read, Seek},
    path::{Path, PathBuf},
};

use zip::{read::ZipFile, result::ZipError};

use crate::error::{AppError, AppResult};

/// Unpacks a zip archive into `dest`, rejecting the whole archive up front if
/// any entry would escape `dest` (zip-slip) or the total uncompressed size
/// would exceed `max_uncompressed_bytes` (zip-bomb). Returns the uncompressed
/// size actually written.
pub fn unpack_zip<R: Read + Seek>(
    reader: R,
    dest: &Path,
    max_uncompressed_bytes: u64,
) -> AppResult<u64> {
    let mut archive = zip::ZipArchive::new(reader)?;

    let mut total_size: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        enclosed_path(&entry)?;
        total_size = total_size.saturating_add(entry.size());
    }
    if total_size > max_uncompressed_bytes {
        return Err(AppError::Zip(ZipError::InvalidArchive(
            "uncompressed size exceeds the configured limit".into(),
        )));
    }

    // Only extract once the whole archive has passed both checks above -
    // never partially unpack an archive that turns out to be unsafe partway
    // through.
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let relative_path = enclosed_path(&entry)?;
        let out_path = dest.join(&relative_path);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out_file = std::fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;
    }

    Ok(total_size)
}

fn enclosed_path<R: Read>(entry: &ZipFile<'_, R>) -> AppResult<PathBuf> {
    entry.enclosed_name().ok_or_else(|| {
        AppError::Zip(ZipError::InvalidArchive(
            "entry path escapes the destination directory".into(),
        ))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::io::Cursor;

    use zip::write::SimpleFileOptions;

    use super::unpack_zip;

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        for (name, contents) in entries {
            writer.start_file(*name, options).expect("start_file");
            std::io::Write::write_all(&mut writer, contents).expect("write contents");
        }
        writer.finish().expect("finish zip").into_inner()
    }

    #[test]
    fn benign_zip_extracts_successfully() {
        let zip_bytes = build_zip(&[("index.html", b"<h1>hi</h1>"), ("css/site.css", b"body {}")]);
        let dest = std::env::temp_dir().join(format!("oxde-test-benign-{}", std::process::id()));
        std::fs::create_dir_all(&dest).expect("create dest");

        let size =
            unpack_zip(Cursor::new(zip_bytes), &dest, 10_000).expect("unpack should succeed");

        assert_eq!(size, 11 + 7);
        assert_eq!(
            std::fs::read_to_string(dest.join("index.html")).expect("read index.html"),
            "<h1>hi</h1>"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("css/site.css")).expect("read site.css"),
            "body {}"
        );

        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn zip_slip_entry_is_rejected() {
        let zip_bytes = build_zip(&[("../../evil.txt", b"pwned")]);
        let dest = std::env::temp_dir().join(format!("oxde-test-slip-{}", std::process::id()));
        std::fs::create_dir_all(&dest).expect("create dest");

        let result = unpack_zip(Cursor::new(zip_bytes), &dest, 10_000);

        assert!(result.is_err(), "zip-slip entry must be rejected");
        assert!(
            !dest.parent().unwrap().join("evil.txt").exists(),
            "entry must never be written outside dest"
        );

        std::fs::remove_dir_all(&dest).ok();
    }

    #[test]
    fn oversized_archive_is_rejected() {
        let zip_bytes = build_zip(&[("big.txt", &vec![b'a'; 1000])]);
        let dest = std::env::temp_dir().join(format!("oxde-test-bomb-{}", std::process::id()));
        std::fs::create_dir_all(&dest).expect("create dest");

        let result = unpack_zip(Cursor::new(zip_bytes), &dest, 10);

        assert!(
            result.is_err(),
            "archive over the uncompressed budget must be rejected"
        );
        assert!(!dest.join("big.txt").exists());

        std::fs::remove_dir_all(&dest).ok();
    }
}
