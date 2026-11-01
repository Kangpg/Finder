use crate::index::Index;
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};
const MAGIC: &[u8; 8] = b"REAIDX01";
const MAX_BYTES: u64 = 512 * 1024 * 1024;

pub fn save(index: &Index, path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    temp.write_all(&[0; 48])?;
    let (length, digest) = {
        let mut writer = BudgetWriter {
            file: temp.as_file_mut(),
            size: 0,
            hash: blake3::Hasher::new(),
        };
        {
            let mut buffered = BufWriter::with_capacity(64 * 1024, &mut writer);
            bincode::serde::encode_into_std_write(
                index,
                &mut buffered,
                bincode::config::standard(),
            )?;
            buffered.flush()?;
        }
        writer.flush()?;
        (writer.size, writer.hash.finalize())
    };
    temp.seek(SeekFrom::Start(0))?;
    temp.write_all(MAGIC)?;
    temp.write_all(&length.to_le_bytes())?;
    temp.write_all(digest.as_bytes())?;
    temp.as_file().sync_all()?;
    temp.persist(path).with_context(|| {
        crate::i18n::Message::capture(|| format!("Cannot replace snapshot {}", path.display()))
    })?;
    Ok(())
}
struct BudgetWriter<'a> {
    file: &'a mut File,
    size: u64,
    hash: blake3::Hasher,
}
impl Write for BudgetWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !self
            .size
            .checked_add(bytes.len() as u64)
            .is_some_and(|n| n <= MAX_BYTES)
        {
            return Err(std::io::Error::other(
                "Snapshot exceeds 512 MB storage budget",
            ));
        }
        let n = self.file.write(bytes)?;
        self.hash.update(&bytes[..n]);
        self.size += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
pub fn load(path: &Path, epoch: Option<(&std::sync::atomic::AtomicU64, u64)>) -> Result<Index> {
    crate::trigram::check(epoch)?;
    let mut file = File::open(path).with_context(|| {
        crate::i18n::Message::capture(|| format!("Cannot open snapshot {}", path.display()))
    })?;
    ensure!(
        file.metadata()?.len() <= MAX_BYTES + 48,
        "Snapshot exceeds storage budget"
    );
    let mut header = [0u8; 48];
    file.read_exact(&mut header)?;
    ensure!(&header[..8] == MAGIC, "Unknown snapshot format");
    let length = u64::from_le_bytes(header[8..16].try_into().unwrap());
    ensure!(length <= MAX_BYTES, "Invalid snapshot length");
    ensure!(
        file.metadata()?.len() == length + 48,
        "Truncated snapshot or trailing data"
    );
    let mut data = Vec::new();
    data.try_reserve_exact(length as usize)?;
    file.take(length + 1).read_to_end(&mut data)?;
    ensure!(
        data.len() as u64 == length,
        "Snapshot length changed while reading"
    );
    ensure!(
        blake3::hash(&data).as_bytes() == &header[16..48],
        "Snapshot checksum mismatch"
    );
    let (mut index, consumed): (Index, usize) = bincode::serde::decode_from_slice(
        &data,
        bincode::config::standard().with_limit::<{ MAX_BYTES as usize }>(),
    )?;
    ensure!(
        consumed == data.len(),
        "Unexpected trailing snapshot content"
    );
    index.validate(epoch)?;
    Ok(index)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_folder_roundtrip_corruption_and_atomic_replace() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("한국 폴더")).unwrap();
        std::fs::write(dir.path().join("한국 폴더/test.rs"), "fixture").unwrap();
        let index = crate::folder::scan(dir.path(), None).unwrap();
        assert_eq!(index.len(), 2);
        let path = dir.path().join("cache.reaidx");
        save(&index, &path).unwrap();
        save(&index, &path).unwrap();
        let loaded = load(&path, None).unwrap();
        let r = loaded
            .search(
                &crate::query::Query::parse("ext:rs", false).unwrap(),
                10,
                None,
            )
            .unwrap();
        assert_eq!(r.total, 1);
        assert!(loaded.path(r.ids[0]).unwrap().is_file());
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[48] ^= 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(load(&path, None).is_err());
        bytes.truncate(20);
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path, None).is_err());
    }
}
