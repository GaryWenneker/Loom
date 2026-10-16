//! One canonical validator shared by owned trees and allocation-light read views.
use crate::format::{decode_object, TREE};
use crate::tree::{validate_entry_parts, MAX_ENTRIES, MAX_NAME};
use crate::{Error, ObjectId, Result, TreeEntryKind};

pub(crate) struct Entry<'a> {
    pub name: &'a str,
    pub kind: TreeEntryKind,
    pub executable: bool,
    pub object: ObjectId,
}

fn name_range(payload: &[u8], offset: u32) -> std::ops::Range<usize> {
    let start = offset as usize + 6;
    let len = u32::from_le_bytes(
        payload[start - 4..start]
            .try_into()
            .expect("validated tree length"),
    ) as usize;
    start..start + len
}
fn name(payload: &[u8], offset: u32) -> &str {
    std::str::from_utf8(&payload[name_range(payload, offset)]).expect("validated immutable UTF-8")
}
fn entry(payload: &[u8], offset: u32) -> Entry<'_> {
    let range = name_range(payload, offset);
    let mut id = [0; 32];
    id.copy_from_slice(&payload[range.end..range.end + 32]);
    Entry {
        name: name(payload, offset),
        kind: if payload[offset as usize] == 1 {
            TreeEntryKind::File
        } else {
            TreeEntryKind::Directory
        },
        executable: payload[offset as usize + 1] == 1,
        object: ObjectId(id),
    }
}

pub(crate) struct TreeData<'a> {
    payload: &'a [u8],
    offsets: Vec<u32>,
}
impl<'a> TreeData<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self> {
        let (kind, payload) = decode_object(bytes)?;
        if kind != TREE {
            return Err(Error::Invalid("expected native tree object".into()));
        }
        fn take<'a>(data: &'a [u8], cursor: &mut usize, size: usize) -> Result<&'a [u8]> {
            if size > data.len().saturating_sub(*cursor) {
                return Err(Error::Invalid("truncated native tree".into()));
            }
            let start = *cursor;
            *cursor += size;
            Ok(&data[start..*cursor])
        }
        let mut cursor = 0;
        let count = u32::from_le_bytes(
            take(payload, &mut cursor, 4)?
                .try_into()
                .expect("four bytes"),
        ) as usize;
        // A valid entry needs flags(2), length(4), nonempty name(1), ID(32).
        // Check before reserving, so a tiny malicious count cannot force a large allocation.
        if count > MAX_ENTRIES || count > payload.len().saturating_sub(4) / 39 {
            return Err(Error::Invalid(
                "too many or truncated native tree entries".into(),
            ));
        }
        let mut offsets = Vec::with_capacity(count);
        let mut previous = None;
        for _ in 0..count {
            let start = cursor;
            let kind = match take(payload, &mut cursor, 1)?[0] {
                1 => TreeEntryKind::File,
                2 => TreeEntryKind::Directory,
                _ => return Err(Error::Invalid("unknown native tree entry kind".into())),
            };
            let executable = match take(payload, &mut cursor, 1)?[0] {
                0 => false,
                1 => true,
                _ => return Err(Error::Invalid("invalid executable flag".into())),
            };
            let len = u32::from_le_bytes(
                take(payload, &mut cursor, 4)?
                    .try_into()
                    .expect("four bytes"),
            ) as usize;
            if len > MAX_NAME {
                return Err(Error::Invalid("oversized native tree entry name".into()));
            }
            let name = std::str::from_utf8(take(payload, &mut cursor, len)?)
                .map_err(|_| Error::Invalid("tree name is not UTF-8".into()))?;
            take(payload, &mut cursor, 32)?;
            validate_entry_parts(name, kind, executable)?;
            if previous.is_some_and(|previous: &str| previous >= name) {
                return Err(Error::Invalid("unsorted or duplicate tree entry".into()));
            }
            previous = Some(name);
            offsets.push(start as u32);
        }
        if cursor != payload.len() {
            return Err(Error::Invalid("trailing native tree bytes".into()));
        }
        Ok(Self { payload, offsets })
    }
    pub fn entries(&self) -> impl Iterator<Item = Entry<'_>> {
        self.offsets
            .iter()
            .map(|&offset| entry(self.payload, offset))
    }
}

/// Sealed immutable bytes and validated offsets. Names borrow the original
/// buffer; querying a directory does not allocate a String per entry.
pub(crate) struct IndexedTree {
    bytes: Vec<u8>,
    payload_start: usize,
    offsets: Vec<u32>,
}
impl IndexedTree {
    pub fn decode(bytes: Vec<u8>) -> Result<Self> {
        let parsed = TreeData::decode(&bytes)?;
        let payload_start = bytes.len() - parsed.payload.len();
        let offsets = parsed.offsets;
        Ok(Self {
            bytes,
            payload_start,
            offsets,
        })
    }
    pub fn is_empty(&self) -> bool {
        self.offsets.is_empty()
    }
    pub fn accounted_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.bytes.capacity()
            + self.offsets.capacity() * std::mem::size_of::<u32>()
    }
    pub fn lookup(&self, component: &str) -> Option<Entry<'_>> {
        let payload = &self.bytes[self.payload_start..];
        let index = self
            .offsets
            .binary_search_by(|&offset| name(payload, offset).cmp(component))
            .ok()?;
        Some(entry(payload, self.offsets[index]))
    }
    pub fn entries(&self) -> impl Iterator<Item = Entry<'_>> {
        self.offsets
            .iter()
            .map(|&offset| entry(&self.bytes[self.payload_start..], offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NativeTree, TreeEntry};
    #[test]
    fn indexed_lookup_preserves_unicode_flags_ids_and_component_boundaries() {
        let original = NativeTree {
            entries: ["a", "a.txt", "é", "中", "😀"]
                .iter()
                .enumerate()
                .map(|(n, name)| TreeEntry {
                    name: (*name).into(),
                    kind: if n == 0 {
                        TreeEntryKind::Directory
                    } else {
                        TreeEntryKind::File
                    },
                    executable: n == 2,
                    object: ObjectId([n as u8; 32]),
                })
                .collect(),
        };
        let bytes = original.encode().unwrap();
        let index = IndexedTree::decode(bytes.clone()).unwrap();
        for expected in &original.entries {
            let entry = index.lookup(&expected.name).unwrap();
            assert_eq!(entry.name, expected.name);
            assert_eq!(entry.kind, expected.kind);
            assert_eq!(entry.executable, expected.executable);
            assert_eq!(entry.object, expected.object);
        }
        assert!(index.lookup("a/child").is_none());
        assert!(index.lookup("éx").is_none());
        assert_eq!(NativeTree::decode(&bytes).unwrap(), original);
        for len in 0..bytes.len() {
            assert!(IndexedTree::decode(bytes[..len].to_vec()).is_err());
        }
        let bad = crate::format::encode_object(TREE, &1_000_000u32.to_le_bytes()).unwrap();
        assert!(IndexedTree::decode(bad).is_err());
    }
}
