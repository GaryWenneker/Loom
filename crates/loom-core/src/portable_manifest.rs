//! Canonical v2 portable file metadata.
//!
//! Manifests are additive immutable objects. Publishing one does not rewrite
//! v1 history, update a ref or opt a repository into a new format.
use crate::format::{decode_object, encode_object, ObjectId, BLOB_MANIFEST};
use crate::{Error, Result};

pub const PORTABLE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PORTABLE_CHUNKS: usize = 16_384;
pub const MAX_SYMLINK_BYTES: usize = 4_096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableFileKind {
    Regular = 1,
    Executable = 2,
    Symlink = 3,
}

impl PortableFileKind {
    fn decode(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Self::Regular),
            2 => Ok(Self::Executable),
            3 => Ok(Self::Symlink),
            _ => Err(Error::Invalid("unknown portable file kind".into())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortableBlobManifest {
    pub kind: PortableFileKind,
    pub logical_bytes: u64,
    pub chunks: Vec<ObjectId>,
}

impl PortableBlobManifest {
    fn validate_shape(&self) -> Result<()> {
        if self.chunks.len() > MAX_PORTABLE_CHUNKS {
            return Err(Error::Invalid("portable manifest has too many chunks".into()));
        }
        let expected = if self.logical_bytes == 0 {
            0
        } else {
            self.logical_bytes
                .div_ceil(PORTABLE_CHUNK_BYTES as u64)
                .try_into()
                .map_err(|_| Error::Invalid("portable manifest is too large".into()))?
        };
        if expected != self.chunks.len() {
            return Err(Error::Invalid(
                "portable manifest length and chunk count disagree".into(),
            ));
        }
        if self.kind == PortableFileKind::Symlink
            && (self.logical_bytes == 0
                || self.logical_bytes > MAX_SYMLINK_BYTES as u64
                || self.chunks.len() != 1)
        {
            return Err(Error::Invalid(
                "portable symlink target must occupy one bounded nonempty chunk".into(),
            ));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        self.validate_shape()?;
        let mut payload = Vec::with_capacity(17 + self.chunks.len() * 32);
        payload.push(self.kind as u8);
        payload.extend_from_slice(&(PORTABLE_CHUNK_BYTES as u32).to_le_bytes());
        payload.extend_from_slice(&self.logical_bytes.to_le_bytes());
        payload.extend_from_slice(&(self.chunks.len() as u32).to_le_bytes());
        for chunk in &self.chunks {
            payload.extend_from_slice(&chunk.0);
        }
        encode_object(BLOB_MANIFEST, &payload)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let (kind, payload) = decode_object(bytes)?;
        if kind != BLOB_MANIFEST {
            return Err(Error::Invalid("expected portable blob manifest".into()));
        }
        if payload.len() < 17 {
            return Err(Error::Invalid("truncated portable blob manifest".into()));
        }
        let file_kind = PortableFileKind::decode(payload[0])?;
        let chunk_bytes = u32::from_le_bytes(payload[1..5].try_into().expect("four bytes"));
        if chunk_bytes as usize != PORTABLE_CHUNK_BYTES {
            return Err(Error::Invalid("noncanonical portable chunk size".into()));
        }
        let logical_bytes = u64::from_le_bytes(payload[5..13].try_into().expect("eight bytes"));
        let count = u32::from_le_bytes(payload[13..17].try_into().expect("four bytes")) as usize;
        if count > MAX_PORTABLE_CHUNKS
            || payload.len().saturating_sub(17) != count.saturating_mul(32)
        {
            return Err(Error::Invalid(
                "portable manifest chunk table is invalid".into(),
            ));
        }
        let chunks = payload[17..]
            .chunks_exact(32)
            .map(|raw| {
                let mut id = [0; 32];
                id.copy_from_slice(raw);
                ObjectId(id)
            })
            .collect();
        let manifest = Self {
            kind: file_kind,
            logical_bytes,
            chunks,
        };
        manifest.validate_shape()?;
        Ok(manifest)
    }
}

pub(crate) fn validate_symlink_target(target: &[u8]) -> Result<()> {
    let target = std::str::from_utf8(target)
        .map_err(|_| Error::Invalid("portable symlink target must be UTF-8".into()))?;
    if target.is_empty()
        || target.starts_with('/')
        || target.starts_with('\\')
        || target.contains('\\')
        || target.contains('\0')
        || target.split('/').any(|part| part.is_empty() || part == "..")
    {
        return Err(Error::Invalid(
            "portable symlink target must be a safe relative UTF-8 path".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_stable_v2_golden_vector_and_rejects_alternate_chunking() {
        let manifest = PortableBlobManifest {
            kind: PortableFileKind::Executable,
            logical_bytes: 3,
            chunks: vec![ObjectId([0x5a; 32])],
        };
        let bytes = manifest.encode().unwrap();
        assert_eq!(&bytes[..7], b"LOOM\0\x02\x04");
        assert_eq!(bytes[15], 2);
        assert_eq!(&bytes[16..20], &(PORTABLE_CHUNK_BYTES as u32).to_le_bytes());
        assert_eq!(PortableBlobManifest::decode(&bytes).unwrap(), manifest);

        let mut noncanonical = bytes;
        noncanonical[16..20].copy_from_slice(&1024u32.to_le_bytes());
        assert!(PortableBlobManifest::decode(&noncanonical).is_err());
    }

    #[test]
    fn manifest_bounds_are_checked_before_chunk_allocation() {
        let mut payload = vec![1];
        payload.extend_from_slice(&(PORTABLE_CHUNK_BYTES as u32).to_le_bytes());
        payload.extend_from_slice(&1u64.to_le_bytes());
        payload.extend_from_slice(&u32::MAX.to_le_bytes());
        let encoded = encode_object(BLOB_MANIFEST, &payload).unwrap();
        assert!(PortableBlobManifest::decode(&encoded).is_err());
        assert!(PortableBlobManifest {
            kind: PortableFileKind::Regular,
            logical_bytes: PORTABLE_CHUNK_BYTES as u64 + 1,
            chunks: vec![ObjectId([1; 32])],
        }
        .encode()
        .is_err());
    }

    #[test]
    fn portable_symlink_policy_is_explicit() {
        for valid in [b"file".as_slice(), b"dir/file", b"./file"] {
            validate_symlink_target(valid).unwrap();
        }
        for invalid in [
            b"".as_slice(),
            b"/absolute",
            b"../escape",
            b"dir/../escape",
            b"dir\\file",
            b"bad\0target",
            &[0xff],
        ] {
            assert!(validate_symlink_target(invalid).is_err());
        }
    }
}
