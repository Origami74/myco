//! The one error type every resolution path fails with.

/// Which guard rejected a napplet. Callers switch on the code rather than
/// parsing the message, so failing closed never depends on wording.
///
/// The codes mirror the reference web runtime's, with one addition Myco needs:
/// [`MultiFile`](NappletErrorCode::MultiFile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NappletErrorCode {
    /// The manifest event's id or Schnorr signature did not verify.
    InvalidSignature,
    /// The event is not a NIP-5D manifest, or its tags are malformed.
    InvalidManifest,
    /// The declared aggregate disagrees with the manifest's own `path` tags.
    AggregateMismatch,
    /// A blob's bytes do not hash to the sha256 its `path` tag claims.
    BlobHashMismatch,
    /// No source served a blob the manifest references.
    BlobUnavailable,
    /// The manifest lists no `/index.html`.
    MissingIndex,
    /// The manifest describes more than one file. NIP-5D: "A napplet is a
    /// single self-contained `/index.html`" — see [`crate::manifest`].
    MultiFile,
}

impl NappletErrorCode {
    /// The stable wire spelling, for logs and (later) the shell's error channel.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InvalidSignature => "invalid-signature",
            Self::InvalidManifest => "invalid-manifest",
            Self::AggregateMismatch => "aggregate-mismatch",
            Self::BlobHashMismatch => "blob-hash-mismatch",
            Self::BlobUnavailable => "blob-unavailable",
            Self::MissingIndex => "missing-index",
            Self::MultiFile => "multi-file",
        }
    }
}

impl std::fmt::Display for NappletErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A napplet failed to parse, verify, or resolve. Every variant is fatal: there
/// is no partial load, and no path from here to an iframe.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{code}: {message}")]
pub struct NappletError {
    pub code: NappletErrorCode,
    pub message: String,
}

impl NappletError {
    pub fn new(code: NappletErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn invalid_manifest(message: impl Into<String>) -> Self {
        Self::new(NappletErrorCode::InvalidManifest, message)
    }
}

/// Every fallible entry point in this crate returns this.
pub type Result<T> = std::result::Result<T, NappletError>;
