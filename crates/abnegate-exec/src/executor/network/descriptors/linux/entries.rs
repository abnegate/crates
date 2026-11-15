use std::io;

use nix::libc;

/// Where `d_reclen`, the length of one record, sits in a
/// `struct linux_dirent64`: after the 8-byte `d_ino` and `d_off`.
const LENGTH_OFFSET: usize = 16;

/// Where `d_name` starts in a `struct linux_dirent64`: after the 2-byte
/// `d_reclen` and the 1-byte `d_type`.
const NAME_OFFSET: usize = 19;

/// The descriptors one `getdents64` read of `/proc/self/fd` names, in the
/// order it names them, skipping `.` and `..`.
///
/// A record that does not fit the buffer, a name without its terminator, or
/// a name that is not a descriptor number yields one
/// [`io::ErrorKind::InvalidData`] error and ends the entries, so a caller
/// that stops on the error marks nothing it cannot account for.
pub(super) struct Entries<'a> {
    buffer: &'a [u8],
}

impl<'a> Entries<'a> {
    /// The entries `buffer`, the bytes one read returned, holds.
    pub(super) fn new(buffer: &'a [u8]) -> Self {
        Self { buffer }
    }

    /// The name of the record at the front of the buffer, and the length of
    /// the whole record, or `None` where the record is malformed.
    fn front(&self) -> Option<(&'a [u8], usize)> {
        let length = self.buffer.get(LENGTH_OFFSET..NAME_OFFSET - 1)?;
        let length = usize::from(u16::from_ne_bytes([length[0], length[1]]));
        let record = self.buffer.get(..length)?;
        let name = record.get(NAME_OFFSET..)?;
        let end = name.iter().position(|&byte| byte == 0)?;
        Some((&name[..end], length))
    }
}

impl Iterator for Entries<'_> {
    type Item = io::Result<libc::c_int>;

    fn next(&mut self) -> Option<Self::Item> {
        while !self.buffer.is_empty() {
            let Some((name, length)) = self.front() else {
                self.buffer = &[];
                return Some(Err(io::Error::from(io::ErrorKind::InvalidData)));
            };
            self.buffer = &self.buffer[length..];
            if matches!(name, b"." | b"..") {
                continue;
            }
            let Some(descriptor) = number(name) else {
                self.buffer = &[];
                return Some(Err(io::Error::from(io::ErrorKind::InvalidData)));
            };
            return Some(Ok(descriptor));
        }
        None
    }
}

/// The descriptor `name` spells in decimal, or `None` where it spells none.
fn number(name: &[u8]) -> Option<libc::c_int> {
    if name.is_empty() {
        return None;
    }
    name.iter().try_fold(0 as libc::c_int, |number, &byte| {
        let digit = byte.checked_sub(b'0').filter(|digit| *digit < 10)?;
        number
            .checked_mul(10)?
            .checked_add(libc::c_int::from(digit))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One record as the kernel writes it, padded to 8 bytes.
    fn record(name: &[u8]) -> Vec<u8> {
        let length = (NAME_OFFSET + name.len() + 1).next_multiple_of(8);
        let mut record = vec![0; length];
        record[LENGTH_OFFSET..NAME_OFFSET - 1]
            .copy_from_slice(&u16::try_from(length).expect("a short name").to_ne_bytes());
        record[NAME_OFFSET..NAME_OFFSET + name.len()].copy_from_slice(name);
        record
    }

    fn listing(names: &[&[u8]]) -> Vec<u8> {
        names.iter().flat_map(|name| record(name)).collect()
    }

    fn read(buffer: &[u8]) -> Vec<io::Result<libc::c_int>> {
        Entries::new(buffer).collect()
    }

    fn invalid(entry: &io::Result<libc::c_int>) -> bool {
        matches!(entry, Err(error) if error.kind() == io::ErrorKind::InvalidData)
    }

    #[test]
    fn a_listing_names_every_descriptor_in_order_and_skips_the_dots() {
        let buffer = listing(&[b".", b"..", b"0", b"1", b"2", b"17", b"2147483647"]);

        let descriptors: Vec<libc::c_int> = Entries::new(&buffer)
            .collect::<io::Result<_>>()
            .expect("a well-formed listing");

        assert_eq!(descriptors, [0, 1, 2, 17, libc::c_int::MAX]);
    }

    #[test]
    fn an_empty_read_names_nothing() {
        assert!(read(&[]).is_empty());
    }

    #[test]
    fn a_record_longer_than_the_read_ends_the_entries_with_an_error() {
        let mut buffer = listing(&[b"3", b"4"]);
        buffer.truncate(buffer.len() - 1);

        let entries = read(&buffer);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].as_ref().ok(), Some(&3));
        assert!(invalid(&entries[1]));
    }

    #[test]
    fn a_record_shorter_than_its_header_ends_the_entries_with_an_error() {
        let mut buffer = record(b"3");
        buffer[LENGTH_OFFSET..NAME_OFFSET - 1].copy_from_slice(&0u16.to_ne_bytes());

        let entries = read(&buffer);

        assert_eq!(entries.len(), 1);
        assert!(invalid(&entries[0]));
    }

    #[test]
    fn a_read_shorter_than_a_header_ends_the_entries_with_an_error() {
        let entries = read(&[0; LENGTH_OFFSET]);

        assert_eq!(entries.len(), 1);
        assert!(invalid(&entries[0]));
    }

    #[test]
    fn a_name_without_its_terminator_ends_the_entries_with_an_error() {
        let mut buffer = record(b"3");
        let length = buffer.len();
        buffer[NAME_OFFSET..length].fill(b'7');

        let entries = read(&buffer);

        assert_eq!(entries.len(), 1);
        assert!(invalid(&entries[0]));
    }

    #[test]
    fn a_name_that_is_no_descriptor_ends_the_entries_with_an_error() {
        for name in [&b"socket"[..], b"3a", b"-3", b"", b"2147483648"] {
            let buffer = listing(&[name, b"4"]);

            let entries = read(&buffer);

            assert_eq!(entries.len(), 1, "{name:?}");
            assert!(invalid(&entries[0]), "{name:?}");
        }
    }
}
