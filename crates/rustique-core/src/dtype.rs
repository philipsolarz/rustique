//! Native element types.

use std::fmt;

/// The native element types a [`Column`](crate::Column) can store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    Str,
}

impl DType {
    pub const ALL: [DType; 12] = [
        DType::Bool,
        DType::I8,
        DType::I16,
        DType::I32,
        DType::I64,
        DType::U8,
        DType::U16,
        DType::U32,
        DType::U64,
        DType::F32,
        DType::F64,
        DType::Str,
    ];

    /// Lower-case name, as used for `List.strategy` and in error messages.
    pub const fn name(self) -> &'static str {
        match self {
            DType::Bool => "bool",
            DType::I8 => "i8",
            DType::I16 => "i16",
            DType::I32 => "i32",
            DType::I64 => "i64",
            DType::U8 => "u8",
            DType::U16 => "u16",
            DType::U32 => "u32",
            DType::U64 => "u64",
            DType::F32 => "f32",
            DType::F64 => "f64",
            DType::Str => "str",
        }
    }

    /// Inverse of [`DType::name`].
    pub fn from_name(name: &str) -> Option<DType> {
        DType::ALL.into_iter().find(|d| d.name() == name)
    }

    /// `struct`-module format character used for the buffer protocol.
    /// `None` for types without a fixed-size representation.
    pub const fn buffer_format(self) -> Option<&'static str> {
        match self {
            DType::Bool => Some("?"),
            DType::I8 => Some("b"),
            DType::I16 => Some("h"),
            DType::I32 => Some("i"),
            DType::I64 => Some("q"),
            DType::U8 => Some("B"),
            DType::U16 => Some("H"),
            DType::U32 => Some("I"),
            DType::U64 => Some("Q"),
            DType::F32 => Some("f"),
            DType::F64 => Some("d"),
            DType::Str => None,
        }
    }

    /// Parse a `struct`-module format string (as found in `memoryview.format`),
    /// accepting the native-alignment prefix `@` and `=`/`<` on little-endian targets.
    pub fn from_buffer_format(format: &str) -> Option<DType> {
        let body = format
            .strip_prefix('@')
            .or_else(|| {
                if cfg!(target_endian = "little") {
                    format.strip_prefix('<').or_else(|| format.strip_prefix('='))
                } else {
                    format.strip_prefix('>').or_else(|| format.strip_prefix('='))
                }
            })
            .unwrap_or(format);
        let dtype = match body {
            "?" => DType::Bool,
            "b" => DType::I8,
            "h" => DType::I16,
            "i" => DType::I32,
            "q" => DType::I64,
            "B" => DType::U8,
            "H" => DType::U16,
            "I" => DType::U32,
            "Q" => DType::U64,
            "f" => DType::F32,
            "d" => DType::F64,
            // `l`/`L` are 8 bytes on LP64 platforms (Linux/macOS), 4 bytes on Windows.
            "l" if size_of::<std::ffi::c_long>() == 8 => DType::I64,
            "L" if size_of::<std::ffi::c_ulong>() == 8 => DType::U64,
            "l" => DType::I32,
            "L" => DType::U32,
            "n" if size_of::<isize>() == 8 => DType::I64,
            "N" if size_of::<usize>() == 8 => DType::U64,
            _ => return None,
        };
        Some(dtype)
    }

    /// Size in bytes of one element, for fixed-size types.
    pub const fn item_size(self) -> Option<usize> {
        match self {
            DType::Bool | DType::I8 | DType::U8 => Some(1),
            DType::I16 | DType::U16 => Some(2),
            DType::I32 | DType::U32 | DType::F32 => Some(4),
            DType::I64 | DType::U64 | DType::F64 => Some(8),
            DType::Str => None,
        }
    }

    pub const fn is_integer(self) -> bool {
        matches!(
            self,
            DType::I8 | DType::I16 | DType::I32 | DType::I64 | DType::U8 | DType::U16 | DType::U32 | DType::U64
        )
    }

    pub const fn is_signed(self) -> bool {
        matches!(self, DType::I8 | DType::I16 | DType::I32 | DType::I64)
    }

    pub const fn is_float(self) -> bool {
        matches!(self, DType::F32 | DType::F64)
    }

    pub const fn is_numeric(self) -> bool {
        self.is_integer() || self.is_float()
    }

    /// Inclusive value range of an integer type.
    pub const fn int_range(self) -> Option<(i128, i128)> {
        match self {
            DType::I8 => Some((i8::MIN as i128, i8::MAX as i128)),
            DType::I16 => Some((i16::MIN as i128, i16::MAX as i128)),
            DType::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
            DType::I64 => Some((i64::MIN as i128, i64::MAX as i128)),
            DType::U8 => Some((0, u8::MAX as i128)),
            DType::U16 => Some((0, u16::MAX as i128)),
            DType::U32 => Some((0, u32::MAX as i128)),
            DType::U64 => Some((0, u64::MAX as i128)),
            _ => None,
        }
    }

    /// Number of bits of an integer or float type.
    pub const fn bits(self) -> Option<u32> {
        match self.item_size() {
            Some(n) if !matches!(self, DType::Bool) => Some(n as u32 * 8),
            _ => None,
        }
    }
}

impl fmt::Display for DType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for d in DType::ALL {
            assert_eq!(DType::from_name(d.name()), Some(d));
        }
    }

    #[test]
    fn buffer_formats_round_trip() {
        for d in DType::ALL {
            if let Some(f) = d.buffer_format() {
                assert_eq!(DType::from_buffer_format(f), Some(d));
                assert_eq!(DType::from_buffer_format(&format!("@{f}")), Some(d));
            }
        }
        assert_eq!(DType::from_buffer_format("x"), None);
    }

    #[test]
    fn ranges() {
        assert_eq!(DType::I8.int_range(), Some((-128, 127)));
        assert_eq!(DType::U64.int_range(), Some((0, u64::MAX as i128)));
        assert_eq!(DType::F64.int_range(), None);
        assert_eq!(DType::U16.bits(), Some(16));
        assert_eq!(DType::Bool.bits(), None);
    }
}
