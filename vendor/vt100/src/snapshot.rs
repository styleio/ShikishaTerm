// NOTE (vendored patch): a screen's whole state, written out and read back.
//
// For a terminal whose program runs somewhere else and outlives the window
// watching it: the process holding the terminal keeps a parser of its own,
// and a window that comes back takes that parser's state instead of starting
// from a blank screen. Rewriting the screen as escape sequences
// (`contents_formatted`) does not do: it writes what is shown now, and
// loses the normal screen behind an alternate one, the scroll region, origin
// mode and the saved cursor, so what comes after lands somewhere else.
//
// The format is this crate's own and says its version first. It is not the
// in-memory layout: a reader of another version refuses it rather than
// reading it wrong.

/// The version of the format [`Screen::snapshot`](crate::Screen::snapshot)
/// writes
pub const SNAPSHOT_VERSION: u16 = 1;

const MAGIC: &[u8; 4] = b"VTS\0";

/// Why a snapshot could not be read
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    /// Not a snapshot at all
    NotASnapshot,
    /// A snapshot of a version this build does not read
    Version(u16),
    /// It ended before it was whole
    Truncated,
    /// A value no screen could have
    Invalid(&'static str),
}

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotASnapshot => write!(f, "not a screen snapshot"),
            Self::Version(v) => write!(f, "a screen snapshot of version {v}, and this reads {SNAPSHOT_VERSION}"),
            Self::Truncated => write!(f, "the screen snapshot ends before it is whole"),
            Self::Invalid(what) => write!(f, "the screen snapshot has an impossible {what}"),
        }
    }
}

impl std::error::Error for SnapshotError {}

pub(crate) fn header(out: &mut Vec<u8>) {
    out.extend_from_slice(MAGIC);
    put_u16(out, SNAPSHOT_VERSION);
}

pub(crate) fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub(crate) fn put_bool(out: &mut Vec<u8>, v: bool) {
    out.push(u8::from(v));
}

/// Reads a snapshot from the front
pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    /// Past the header, or why not
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, SnapshotError> {
        let mut r = Self { bytes };
        if r.take(4).map_err(|_| SnapshotError::NotASnapshot)? != MAGIC {
            return Err(SnapshotError::NotASnapshot);
        }
        let version = r.u16()?;
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotError::Version(version));
        }
        Ok(r)
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8], SnapshotError> {
        if self.bytes.len() < n {
            return Err(SnapshotError::Truncated);
        }
        let (head, rest) = self.bytes.split_at(n);
        self.bytes = rest;
        Ok(head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, SnapshotError> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u16(&mut self) -> Result<u16, SnapshotError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub(crate) fn u32(&mut self) -> Result<u32, SnapshotError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn bool(&mut self) -> Result<bool, SnapshotError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(SnapshotError::Invalid("flag")),
        }
    }

    /// Whether everything was read: bytes left over mean it was not
    /// understood the way it was written
    pub(crate) fn finish(self) -> Result<(), SnapshotError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(SnapshotError::Invalid("length"))
        }
    }
}
