//! Canonical additive v2 directory nodes over portable blob manifests.

use crate::format::{decode_object, encode_object, ObjectId, PORTABLE_TREE};
use crate::{Error, Result};
use std::collections::BTreeSet;

pub const MAX_PORTABLE_TREE_ENTRIES: usize = 1_000_000;
pub const MAX_PORTABLE_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableTreeEntryKind {
    File = 1,
    Directory = 2,
}

impl PortableTreeEntryKind {
    fn decode(raw: u8) -> Result<Self> {
        match raw {
            1 => Ok(Self::File),
            2 => Ok(Self::Directory),
            _ => Err(Error::Invalid("unknown portable tree entry kind".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTreeEntry {
    pub name: String,
    pub kind: PortableTreeEntryKind,
    pub object: ObjectId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableTree {
    pub entries: Vec<PortableTreeEntry>,
}

fn portable_name_key(name: &str) -> Result<String> {
    if name.is_empty()
        || name.len() > MAX_PORTABLE_NAME_BYTES
        || name == "."
        || name == ".."
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.ends_with(' ')
        || name.ends_with('.')
        || name.chars().any(char::is_control)
    {
        return Err(Error::Invalid("unsafe portable tree name".into()));
    }
    let folded = name.to_ascii_lowercase();
    let stem = folded.split('.').next().unwrap_or(&folded);
    let reserved = matches!(stem, "con" | "prn" | "aux" | "nul")
        || (stem.len() == 4
            && (stem.starts_with("com") || stem.starts_with("lpt"))
            && matches!(stem.as_bytes()[3], b'1'..=b'9'));
    if reserved || matches!(folded.as_str(), ".loom" | ".git") {
        return Err(Error::Invalid("reserved portable tree name".into()));
    }
    Ok(folded)
}

impl PortableTree {
    fn validate(&self) -> Result<()> {
        if self.entries.len() > MAX_PORTABLE_TREE_ENTRIES {
            return Err(Error::Invalid("too many portable tree entries".into()));
        }
        let mut previous: Option<&str> = None;
        let mut portable_keys = BTreeSet::new();
        for entry in &self.entries {
            if previous.is_some_and(|name| name >= entry.name.as_str()) {
                return Err(Error::Invalid(
                    "unsorted or repeated portable tree entry".into(),
                ));
            }
            previous = Some(&entry.name);
            if !portable_keys.insert(portable_name_key(&entry.name)?) {
                return Err(Error::Invalid(
                    "case-insensitive portable tree name collision".into(),
                ));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut payload = Vec::new();
        payload.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for entry in &self.entries {
            payload.push(entry.kind as u8);
            payload.extend_from_slice(&(entry.name.len() as u16).to_le_bytes());
            payload.extend_from_slice(entry.name.as_bytes());
            payload.extend_from_slice(&entry.object.0);
        }
        encode_object(PORTABLE_TREE, &payload)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (kind, payload) = decode_object(bytes)?;
        if kind != PORTABLE_TREE || payload.len() < 4 {
            return Err(Error::Invalid("expected portable tree".into()));
        }
        let count = u32::from_le_bytes(payload[..4].try_into().expect("four bytes")) as usize;
        if count > MAX_PORTABLE_TREE_ENTRIES {
            return Err(Error::Invalid("too many portable tree entries".into()));
        }
        let mut cursor = 4usize;
        let mut entries = Vec::with_capacity(count.min(4096));
        for _ in 0..count {
            let entry_kind = PortableTreeEntryKind::decode(
                *payload
                    .get(cursor)
                    .ok_or_else(|| Error::Invalid("truncated portable tree entry".into()))?,
            )?;
            cursor += 1;
            let raw_len = payload
                .get(cursor..cursor + 2)
                .ok_or_else(|| Error::Invalid("truncated portable tree name".into()))?;
            let name_len = u16::from_le_bytes(raw_len.try_into().expect("two bytes")) as usize;
            cursor += 2;
            let raw_name = payload
                .get(cursor..cursor + name_len)
                .ok_or_else(|| Error::Invalid("truncated portable tree name".into()))?;
            cursor += name_len;
            let name = std::str::from_utf8(raw_name)
                .map_err(|_| Error::Invalid("portable tree name is not UTF-8".into()))?
                .to_owned();
            let raw_id = payload
                .get(cursor..cursor + 32)
                .ok_or_else(|| Error::Invalid("truncated portable tree object ID".into()))?;
            cursor += 32;
            let mut object = [0; 32];
            object.copy_from_slice(raw_id);
            entries.push(PortableTreeEntry {
                name,
                kind: entry_kind,
                object: ObjectId(object),
            });
        }
        if cursor != payload.len() {
            return Err(Error::Invalid("trailing portable tree bytes".into()));
        }
        let tree = Self { entries };
        tree.validate()?;
        Ok(tree)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_tree_has_stable_golden_vector() {
        let tree = PortableTree {
            entries: vec![PortableTreeEntry {
                name: "a".into(),
                kind: PortableTreeEntryKind::File,
                object: ObjectId([0x5a; 32]),
            }],
        };
        let encoded = tree.encode().unwrap();
        assert_eq!(&encoded[..7], b"LOOM\0\x02\x05");
        assert_eq!(&encoded[15..22], &[1, 0, 0, 0, 1, 1, 0]);
        assert_eq!(PortableTree::decode(&encoded).unwrap(), tree);
    }

    #[test]
    fn portable_tree_rejects_cross_platform_collisions_and_reserved_names() {
        for name in ["CON", "aux.txt", "trail.", "trail ", ".git", "a/b"] {
            assert!(
                PortableTree {
                    entries: vec![PortableTreeEntry {
                        name: name.into(),
                        kind: PortableTreeEntryKind::File,
                        object: ObjectId([1; 32]),
                    }],
                }
                .encode()
                .is_err(),
                "{name}"
            );
        }
        let collision = PortableTree {
            entries: vec![
                PortableTreeEntry {
                    name: "A".into(),
                    kind: PortableTreeEntryKind::File,
                    object: ObjectId([1; 32]),
                },
                PortableTreeEntry {
                    name: "a".into(),
                    kind: PortableTreeEntryKind::File,
                    object: ObjectId([2; 32]),
                },
            ],
        };
        assert!(collision.encode().is_err());
    }
}
