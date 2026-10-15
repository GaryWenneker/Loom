//! Canonical, immutable, hierarchical directory nodes.
//!
//! This is a version-2 *object type*, not a migration of the v1 changeset
//! history or a claim that v2 worktree metadata is already preserved.
use crate::format::{decode_object, encode_object, validate_path, ObjectId, TREE};
use crate::{Error, Result};

const MAX_ENTRIES: usize = 1_000_000;
const MAX_NAME: usize = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TreeEntryKind {
    File = 1,
    Directory = 2,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// One path component, never a path with separators.
    pub name: String,
    pub kind: TreeEntryKind,
    /// Reserved for preserving the executable bit; v1 migrations use false.
    pub executable: bool,
    pub object: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeTree {
    /// Lexicographically sorted by UTF-8 bytes; no duplicate names.
    pub entries: Vec<TreeEntry>,
}

fn validate_entry(entry: &TreeEntry) -> Result<()> {
    validate_path(&entry.name)?;
    if entry.name.contains('/') || entry.name.len() > MAX_NAME {
        return Err(Error::Invalid("tree name must be one bounded path component".into()));
    }
    if entry.kind == TreeEntryKind::Directory && entry.executable {
        return Err(Error::Invalid("directory cannot have executable-file flag".into()));
    }
    Ok(())
}

impl NativeTree {
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.entries.len() > MAX_ENTRIES {
            return Err(Error::Invalid("too many tree entries".into()));
        }
        let mut data = Vec::new();
        data.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        let mut previous: Option<&str> = None;
        for entry in &self.entries {
            validate_entry(entry)?;
            if previous.is_some_and(|name| name >= entry.name.as_str()) {
                return Err(Error::Invalid("unsorted or repeated tree entry".into()));
            }
            previous = Some(&entry.name);
            data.push(entry.kind as u8);
            data.push(u8::from(entry.executable));
            data.extend_from_slice(&(entry.name.len() as u32).to_le_bytes());
            data.extend_from_slice(entry.name.as_bytes());
            data.extend_from_slice(&entry.object.0);
        }
        encode_object(TREE, &data)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (kind, payload) = decode_object(bytes)?;
        if kind != TREE {
            return Err(Error::Invalid("expected native tree object".into()));
        }
        let mut cursor = 0usize;
        fn take<'a>(data: &'a [u8], pos: &mut usize, size: usize) -> Result<&'a [u8]> {
            if size > data.len().saturating_sub(*pos) {
                return Err(Error::Invalid("truncated native tree".into()));
            }
            let start = *pos;
            *pos += size;
            Ok(&data[start..*pos])
        }
        let count = u32::from_le_bytes(
            take(payload, &mut cursor, 4)?.try_into().expect("four bytes")
        ) as usize;
        if count > MAX_ENTRIES {
            return Err(Error::Invalid("too many tree entries".into()));
        }
        let mut entries = Vec::new();
        let mut previous = String::new();
        for i in 0..count {
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
                take(payload, &mut cursor, 4)?.try_into().expect("four bytes")
            ) as usize;
            if len > MAX_NAME {
                return Err(Error::Invalid("oversized native tree entry name".into()));
            }
            let name = std::str::from_utf8(take(payload, &mut cursor, len)?)
                .map_err(|_| Error::Invalid("tree name is not UTF-8".into()))?
                .to_owned();
            let mut object = [0; 32];
            object.copy_from_slice(take(payload, &mut cursor, 32)?);
            let entry = TreeEntry { name, kind, executable, object: ObjectId(object) };
            validate_entry(&entry)?;
            if i != 0 && entry.name <= previous {
                return Err(Error::Invalid("unsorted or duplicate tree entry".into()));
            }
            previous = entry.name.clone();
            entries.push(entry);
        }
        if cursor != payload.len() {
            return Err(Error::Invalid("trailing native tree bytes".into()));
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> NativeTree {
        NativeTree { entries: vec![
            TreeEntry { name: "README.md".into(), kind: TreeEntryKind::File,
                        executable: false, object: ObjectId([1; 32]) },
            TreeEntry { name: "src".into(), kind: TreeEntryKind::Directory,
                        executable: false, object: ObjectId([2; 32]) },
        ] }
    }

    #[test]
    fn roundtrip_and_deterministic_identity() {
        let source = tree();
        let bytes = source.encode().unwrap();
        assert_eq!(bytes[5], crate::format::TREE_VERSION);
        assert_eq!(bytes[6], TREE);
        assert_eq!(NativeTree::decode(&bytes).unwrap(), source);
        assert_eq!(crate::format::object_id(&bytes),
                   crate::format::object_id(&source.encode().unwrap()));
        assert_eq!(NativeTree { entries: vec![] }.encode().unwrap().len(), 19);
    }

    #[test]
    fn reject_noncanonical_names_and_directory_flags() {
        let cases = ["../escape", "two/components", ".loom", ".git", "a\\b", ""];
        for name in cases {
            let mut value = tree();
            value.entries[0].name = name.into();
            assert!(value.encode().is_err(), "{name}");
        }
        let mut duplicate = tree();
        duplicate.entries.push(duplicate.entries[1].clone());
        assert!(duplicate.encode().is_err());
        let mut wrong_order = tree();
        wrong_order.entries.reverse();
        assert!(wrong_order.encode().is_err());
        let mut flag = tree();
        flag.entries[1].executable = true;
        assert!(flag.encode().is_err());
    }

    #[test]
    fn reject_noncanonical_serialized_data() {
        let bytes = tree().encode().unwrap();
        for len in [0, 4, 15, bytes.len() - 1] {
            assert!(NativeTree::decode(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(42);
        assert!(NativeTree::decode(&trailing).is_err());
        let mut invalid_flag = bytes.clone();
        invalid_flag[15 + 4 + 1] = 3;
        assert!(NativeTree::decode(&invalid_flag).is_err());
        let mut wrong_kind = bytes.clone();
        wrong_kind[6] = crate::format::BLOB;
        assert!(NativeTree::decode(&wrong_kind).is_err());
    }
}
