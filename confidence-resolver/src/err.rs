use core::panic::Location;

/// A minimal error type suitable as a replacement for runtime panics.
///
/// - Its only state is a 48‑bit code intended to be unique per call site or tag.
/// - Renders as an 8‑char base64url code (no padding).
/// - Use `ErrorCode::from_location()` (#[track_caller]) to derive a code from the call site, or
///   `ErrorCode::from_tag("module.feature.case")` for a stable tag.
/// - Typical internal use: return `Fallible<T>` (alias for `Result<T, ErrorCode>`) and propagate with `?`.
/// - At API boundaries that return `Result<T, String>`, `?` works via `From<ErrorCode> for String`
///   and renders as `internal error [XXXXXXXX]`.
/// - `Option<T>` or `Result<T,_>` can be converted to `Fallible<T>` via `.or_fail()` See `OrFailExt`
///
/// Note: We do not (yet) ship a code→location/tag table; that can be generated in a separate build if needed.
///
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ErrorCode([u8; 6]);

impl ErrorCode {
    pub const fn new(code: u64) -> Self {
        let b = code.to_le_bytes();
        ErrorCode([b[0], b[1], b[2], b[3], b[4], b[5]])
    }

    /// Builds a code from a stable tag string.
    ///
    /// - Useful when you want stability across refactors (line moves).
    /// - Keep tags short and unique within the crate; consider a CI check for duplicates.
    pub const fn from_tag(tag: &str) -> Self {
        ErrorCode::new(fnv1a64([tag.as_bytes()]))
    }

    /// Builds a code from the caller’s file/line/column.
    ///
    /// - Uses `#[track_caller]` so the code reflects the call site (moves if lines change).
    /// - Prefer this where you’d otherwise `panic!`.
    #[track_caller]
    pub const fn from_location() -> Self {
        let loc = Location::caller();
        let parts = [
            loc.file().as_bytes(),
            &loc.line().to_le_bytes(),
            &loc.column().to_le_bytes(),
        ];
        ErrorCode::new(fnv1a64(parts))
    }

    /// Returns the 8‑character base64url encoding of the 48‑bit code (no padding).
    pub fn b64(self) -> [u8; 8] {
        // load to 48-bit value
        let mut v: u64 = 0;
        for b in self.0 {
            v = (v << 8) | (b as u64);
        }

        // emit 8 sextets (MSB first)
        core::array::from_fn(|i| {
            let shift = 42usize.wrapping_sub(i.wrapping_mul(6usize));
            let sextet = ((v >> shift) & 0x3F) as u8;
            b64u6(sextet)
        })
    }

    /// Returns the raw 48‑bit code in the low bits of a `u64` (little‑endian packing).
    pub fn code(self) -> u64 {
        let mut b = [0u8; 8];
        b[..6].copy_from_slice(&self.0);
        u64::from_le_bytes(b)
    }

    pub fn b64_str(self) -> impl core::fmt::Display {
        struct D([u8; 8]);
        impl core::fmt::Display for D {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                // safe because base 64 is ASCII so identical bytes in utf8
                write!(f, "{}", unsafe { core::str::from_utf8_unchecked(&self.0) })
            }
        }
        D(self.b64())
    }
}

impl From<ErrorCode> for String {
    fn from(e: ErrorCode) -> String {
        e.to_string()
    }
}

/// Convenience alias for `Result<T, ErrorCode>` used in internal APIs.
pub type Fallible<T> = core::result::Result<T, ErrorCode>;

/// Extension methods to collapse `Option<T>`/`Result<T, E>` into `Fallible<T>`.
///
/// - `Option<T>::or_fail()` → code from call site if `None`.
/// - `Result<T, E>::or_fail()` → maps any `Err(E)` to a call‑site code.
///   Prefer plain `?` when the error is already `ErrorCode`.
pub trait OrFailExt<T> {
    #[track_caller]
    fn or_fail(self) -> Fallible<T>;
}

/// Macro: derive an `ErrorCode` from a module‑qualified tag.
///
/// - Usage: `module_err!(":subsystem.case")`
/// - Expands to `ErrorCode::from_tag(concat!(module_path!(), tag))`.
/// - Returns an `ErrorCode` value (not a `Result`); use with `ok_or(...)` / `map_err(...)`.
///
/// Examples:
/// ```rust
/// let code = module_err!(":gzip.crc_mismatch");
/// let crc = buf.get(..4).ok_or(module_err!(":gzip.truncated_crc"))?;
/// ```
#[macro_export]
macro_rules! module_err {
    ($tag:literal) => {
        $crate::ErrorCode::from_tag(concat!(module_path!(), $tag))
    };
}

/// Macro: early‑return with `Err(ErrorCode)`.
///
/// Forms:
/// - `fail!()`        → `return Err(ErrorCode::from_location())`
/// - `fail!(":tag")`  → `return Err(module_err!(":tag"))`
///
/// Notes:
/// - Add a semicolon at the call site when used as a statement: `fail!();`.
/// - Prefer `.or_fail()?` on `Option`/`Result` when you’re not immediately returning.
///
/// Examples:
/// ```rust
/// if bad_magic { fail!(":parser.bad_magic"); }
/// let v = maybe_val.or_fail()?; // alternative for Option
/// ```
#[macro_export]
macro_rules! fail {
    () => {
        return Err($crate::ErrorCode::from_location().into())
    };
    ($tag:literal) => {
        return Err($crate::module_err!($tag).into())
    };
}

impl<T> OrFailExt<T> for Option<T> {
    #[track_caller]
    fn or_fail(self) -> Fallible<T> {
        self.ok_or(ErrorCode::from_location())
    }
}

impl<T, E> OrFailExt<T> for Result<T, E> {
    #[track_caller]
    fn or_fail(self) -> Fallible<T> {
        self.map_err(|_| ErrorCode::from_location())
    }
}

impl core::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "internal error [{}]", self.b64_str())
    }
}

/// Unified error type for the resolve path.
///
/// - `Internal(ErrorCode)` — low-level "should never happen" errors (from `.or_fail()?`)
/// - `Message(String)` — descriptive errors for API boundaries
/// - `MissingMaterializations` — flag needs materializations, triggers suspend
/// - `MaterializationsUnsupported` — caller has no materialization support, flag should error
#[derive(Debug, PartialEq)]
pub enum ResolveError {
    Internal(ErrorCode),
    Message(String),
    MissingMaterializations,
    MaterializationsUnsupported,
    UnrecognizedRule,
}

impl ResolveError {
    pub fn message(msg: &str) -> Self {
        ResolveError::Message(msg.to_string())
    }
}

impl From<ErrorCode> for ResolveError {
    fn from(e: ErrorCode) -> Self {
        ResolveError::Internal(e)
    }
}

impl From<ResolveError> for String {
    fn from(e: ResolveError) -> String {
        match e {
            ResolveError::Internal(code) => code.to_string(),
            ResolveError::Message(msg) => msg,
            ResolveError::MissingMaterializations => "Missing materializations".to_string(),
            ResolveError::MaterializationsUnsupported => {
                "Materializations not supported".to_string()
            }
            ResolveError::UnrecognizedRule => "Unrecognized rule".to_string(),
        }
    }
}

impl core::fmt::Display for ResolveError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ResolveError::Internal(code) => write!(f, "{}", code),
            ResolveError::Message(msg) => write!(f, "{}", msg),
            ResolveError::MissingMaterializations => write!(f, "Missing materializations"),
            ResolveError::MaterializationsUnsupported => {
                write!(f, "Materializations not supported")
            }
            ResolveError::UnrecognizedRule => write!(f, "Unrecognized rule"),
        }
    }
}

#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
const fn fnv1a64<const N: usize>(parts: [&[u8]; N]) -> u64 {
    const FNV64_INIT: u64 = 0xCBF2_9CE4_8422_2325;
    const FNV64_PRIME: u64 = 0x1000_0000_01B3;

    let mut h = FNV64_INIT;
    let mut i = 0;
    while i < N {
        let b = parts[i];
        let mut j = 0;
        while j < b.len() {
            h ^= b[j] as u64;
            h = h.wrapping_mul(FNV64_PRIME);
            j += 1;
        }
        i += 1;
    }
    h
}

#[inline]
#[allow(clippy::arithmetic_side_effects)]
fn b64u6(x: u8) -> u8 {
    match x {
        0..=25 => b'A' + x,
        26..=51 => b'a' + (x - 26),
        52..=61 => b'0' + (x - 52),
        62 => b'-',
        _ => b'_', // 63
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display() {
        let e = ErrorCode::from_location();
        let s = e.to_string();
        assert!(s.starts_with("internal error ["));
    }

    #[test]
    fn different_call_sites_differ() {
        let a = ErrorCode::from_location();
        let b = ErrorCode::from_location(); // different line ⇒ different site
        assert_ne!(a, b);
    }
}
