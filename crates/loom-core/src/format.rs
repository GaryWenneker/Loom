use crate::{Error, Result};
use std::fmt;

/// Canonical v1 object envelope: "LOOM\\0", version, kind, u64 payload length LE, payload.
pub const MAGIC: &[u8; 5] = b"LOOM\0";
pub const VERSION: u8 = 1;
pub const BLOB: u8 = 1;
pub const CHANGESET: u8 = 2;
/// Version-2 immutable directory tree nodes, independent of v1 changesets.
pub const TREE: u8 = 3;
pub const TREE_VERSION: u8 = 2;
pub const MAX_OBJECT_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectId(pub [u8; 32]);
impl ObjectId {
    pub fn parse(hex: &str) -> Result<Self> {
        if hex.len() != 64 || !hex.is_ascii() {
            return Err(Error::Invalid(
                "object ID must be 64 ASCII hex digits".into(),
            ));
        }
        fn nibble(byte: u8) -> Result<u8> {
            match byte {
                b'0'..=b'9' => Ok(byte - b'0'),
                b'a'..=b'f' => Ok(byte - b'a' + 10),
                b'A'..=b'F' => Ok(byte - b'A' + 10),
                _ => Err(Error::Invalid("invalid object ID hex".into())),
            }
        }
        let mut b = [0u8; 32];
        for (byte, pair) in b.iter_mut().zip(hex.as_bytes().chunks_exact(2)) {
            *byte = (nibble(pair[0])? << 4) | nibble(pair[1])?;
        }
        Ok(Self(b))
    }
}
impl fmt::Display for ObjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut text = [0; 64];
        for (byte, pair) in self.0.iter().zip(text.chunks_exact_mut(2)) {
            pair[0] = HEX[(byte >> 4) as usize];
            pair[1] = HEX[(byte & 15) as usize];
        }
        f.write_str(std::str::from_utf8(&text).expect("ASCII hexadecimal"))
    }
}

pub(crate) fn object_header(kind: u8, length: usize) -> Result<[u8; 15]> {
    if length > MAX_OBJECT_SIZE {
        return Err(Error::Invalid("object exceeds 64 MiB v1 cap".into()));
    }
    if kind != BLOB && kind != CHANGESET && kind != TREE {
        return Err(Error::Invalid("unknown object kind".into()));
    }
    let mut header = [0; 15];
    header[..5].copy_from_slice(MAGIC);
    header[5] = if kind == TREE { TREE_VERSION } else { VERSION };
    header[6] = kind;
    header[7..].copy_from_slice(&(length as u64).to_le_bytes());
    Ok(header)
}
pub(crate) fn encode_object(kind: u8, payload: &[u8]) -> Result<Vec<u8>> {
    let header = object_header(kind, payload.len())?;
    let mut b = Vec::with_capacity(payload.len() + header.len());
    b.extend_from_slice(&header);
    b.extend_from_slice(payload);
    Ok(b)
}
pub(crate) fn decode_object(b: &[u8]) -> Result<(u8, &[u8])> {
    if b.len() < 15 || &b[..5] != MAGIC {
        return Err(Error::Invalid("unrecognized object envelope".into()));
    }
    let kind = b[6];
    if !matches!(
        (b[5], kind),
        (VERSION, BLOB | CHANGESET) | (TREE_VERSION, TREE)
    ) {
        return Err(Error::Invalid("unsupported object version or kind".into()));
    }
    let len = u64::from_le_bytes(b[7..15].try_into().expect("8 bytes"));
    if len > MAX_OBJECT_SIZE as u64 || len != (b.len() - 15) as u64 {
        return Err(Error::Invalid("object payload length mismatch".into()));
    }
    Ok((kind, &b[15..]))
}
pub(crate) fn object_id(envelope: &[u8]) -> ObjectId {
    ObjectId(*blake3::hash(envelope).as_bytes())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Add = 1,
    Modify = 2,
    Delete = 3,
}
impl ChangeKind {
    fn parse(n: u8) -> Result<Self> {
        match n {
            1 => Ok(Self::Add),
            2 => Ok(Self::Modify),
            3 => Ok(Self::Delete),
            _ => Err(Error::Invalid("bad change kind".into())),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    pub path: String,
    pub blob: Option<ObjectId>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changeset {
    /// First parent defines the state against which changes are applied.
    pub parents: Vec<ObjectId>,
    pub author: String,
    pub message: String,
    pub timestamp_seconds: i64,
    pub changes: Vec<Change>,
}
fn put_string(dst: &mut Vec<u8>, s: &str) -> Result<()> {
    let n: u32 = s
        .len()
        .try_into()
        .map_err(|_| Error::Invalid("string too long".into()))?;
    dst.extend_from_slice(&n.to_le_bytes());
    dst.extend_from_slice(s.as_bytes());
    Ok(())
}
impl Changeset {
    pub(crate) fn encode(&self) -> Result<Vec<u8>> {
        if self.parents.len() > 2 || self.changes.len() > 1_000_000 {
            return Err(Error::Invalid("too many parents or changes".into()));
        }
        if self.author.len() > 1_000_000 || self.message.len() > 1_000_000 {
            return Err(Error::Invalid("metadata too long".into()));
        }
        let mut p = Vec::new();
        p.push(self.parents.len() as u8);
        for parent in &self.parents {
            p.extend_from_slice(&parent.0);
        }
        p.extend_from_slice(&self.timestamp_seconds.to_le_bytes());
        put_string(&mut p, &self.author)?;
        put_string(&mut p, &self.message)?;
        let mut sorted = self.changes.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        p.extend_from_slice(&(sorted.len() as u32).to_le_bytes());
        let mut prev: Option<&str> = None;
        for c in &sorted {
            validate_path(&c.path)?;
            if prev == Some(c.path.as_str()) {
                return Err(Error::Invalid("duplicate path".into()));
            }
            prev = Some(&c.path);
            p.push(c.kind as u8);
            put_string(&mut p, &c.path)?;
            if c.kind == ChangeKind::Delete {
                if c.blob.is_some() {
                    return Err(Error::Invalid("delete must not reference a blob".into()));
                }
            } else {
                p.extend_from_slice(
                    &c.blob
                        .ok_or_else(|| Error::Invalid("missing blob".into()))?
                        .0,
                );
            }
        }
        encode_object(CHANGESET, &p)
    }
    pub(crate) fn decode(data: &[u8]) -> Result<Self> {
        let (kind, payload) = decode_object(data)?;
        if kind != CHANGESET {
            return Err(Error::Invalid("expected changeset".into()));
        }
        let mut r = Reader { b: payload, pos: 0 };
        let n = r.byte()? as usize;
        if n > 2 {
            return Err(Error::Invalid("too many parents".into()));
        }
        let mut parents = Vec::with_capacity(n);
        for _ in 0..n {
            let mut b = [0; 32];
            b.copy_from_slice(r.take(32)?);
            parents.push(ObjectId(b));
        }
        let timestamp_seconds = i64::from_le_bytes(r.take(8)?.try_into().expect("8 bytes"));
        let author = r.string()?;
        let message = r.string()?;
        let count = r.u32()? as usize;
        if count > 1_000_000 {
            return Err(Error::Invalid("too many changes".into()));
        }
        let mut changes = Vec::with_capacity(count);
        let mut prev = String::new();
        for i in 0..count {
            let kind = ChangeKind::parse(r.byte()?)?;
            let path = r.string()?;
            validate_path(&path)?;
            if i > 0 && path <= prev {
                return Err(Error::Invalid("unsorted changes".into()));
            }
            prev = path.clone();
            let blob = if kind == ChangeKind::Delete {
                None
            } else {
                let mut id = [0; 32];
                id.copy_from_slice(r.take(32)?);
                Some(ObjectId(id))
            };
            changes.push(Change { kind, path, blob });
        }
        if r.pos != payload.len() {
            return Err(Error::Invalid("trailing object bytes".into()));
        }
        Ok(Self {
            parents,
            author,
            message,
            timestamp_seconds,
            changes,
        })
    }
}
pub(crate) fn validate_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == ".." || p == ".loom" || p == ".git")
    {
        return Err(Error::Invalid(format!("unsafe repository path: {path:?}")));
    }
    Ok(())
}
struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if n > self.b.len().saturating_sub(self.pos) {
            return Err(Error::Invalid("truncated object".into()));
        }
        let out = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        if n > 1_000_000 {
            return Err(Error::Invalid("string too long".into()));
        }
        Ok(std::str::from_utf8(self.take(n)?)
            .map_err(|_| Error::Invalid("invalid UTF-8".into()))?
            .to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_and_stable_id() {
        let c = Changeset {
            parents: vec![],
            author: "test".into(),
            message: "hello".into(),
            timestamp_seconds: 1,
            changes: vec![Change {
                kind: ChangeKind::Add,
                path: "a.txt".into(),
                blob: Some(ObjectId([7; 32])),
            }],
        };
        let bytes = c.encode().unwrap();
        assert_eq!(Changeset::decode(&bytes).unwrap(), c);
        assert_eq!(object_id(&bytes), object_id(&c.encode().unwrap()));
    }
    #[test]
    fn reject_unsafe_paths() {
        for p in [
            "../z",
            "/z",
            "x/../y",
            "a\\b",
            ".loom/index",
            "x//y",
            "x/.git/config",
        ] {
            assert!(validate_path(p).is_err(), "{p}");
        }
    }
    #[test]
    fn reject_truncated_object() {
        assert!(Changeset::decode(b"LOOM").is_err());
    }
    #[test]
    fn object_id_text_preserves_all_bytes_and_rejects_non_hex_aliases() {
        let id = ObjectId(std::array::from_fn(|n| n as u8));
        assert_eq!(
            id.to_string(),
            "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f"
        );
        for byte in 0..=255 {
            let id = ObjectId([byte; 32]);
            let expected = format!("{byte:02x}").repeat(32);
            assert_eq!(id.to_string(), expected);
            assert_eq!(ObjectId::parse(&expected).unwrap(), id);
            assert_eq!(ObjectId::parse(&expected.to_uppercase()).unwrap(), id);
        }
        for byte in 0u8..=127 {
            if byte.is_ascii_hexdigit() {
                continue;
            }
            for position in [0, 31, 63] {
                let mut text = vec![b'0'; 64];
                text[position] = byte;
                assert!(ObjectId::parse(std::str::from_utf8(&text).unwrap()).is_err());
            }
        }
        for invalid in [
            "+f".repeat(32),
            "é".repeat(32),
            "0".repeat(63),
            "0".repeat(65),
        ] {
            assert!(ObjectId::parse(&invalid).is_err());
        }
    }
}
