use std::{fs::File, io};

cfg_select! {
    unix => {
        pub(crate) fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
            std::os::unix::fs::FileExt::read_at(file, bytes, offset)
        }
    },
    windows => {
        pub(crate) fn read_at(file: &File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
            std::os::windows::fs::FileExt::seek_read(file, bytes, offset)
        }
    },
    _ => {
        pub(crate) fn read_at(_file: &File, _bytes: &mut [u8], _offset: u64) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "file-backed HDP volumes are unavailable on this platform"))
        }
    }
}

pub(crate) fn read_exact_at(file: &File, mut bytes: &mut [u8], mut offset: u64) -> io::Result<()> {
    while !bytes.is_empty() {
        match read_at(file, bytes, offset) {
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Ok(count) => {
                bytes = &mut bytes[count..];
                offset = offset
                    .checked_add(count as u64)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}
