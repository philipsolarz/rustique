use pyo3::prelude::*;

pub mod i8;   // i8 type
pub mod i16;  // i16 type
pub mod i32;  // i32 type
pub mod i64;  // i64 type
pub mod i128;  // i128 type
pub mod u8;   // u8 type
pub mod u16;  // u16 type
pub mod u32;  // u32 type
pub mod u64;  // u64 type
pub mod u128;  // u128 type
pub mod isize;  // isize type
pub mod usize;  // usize type
pub mod bool;  // bool type
pub mod char;  // char type
pub mod str;   // str type
pub mod f32;  // f32 type
pub mod f64;  // f64 type
pub mod int;
pub mod float;


/// Register all primitive types with the Python module
pub fn register_primitives(m: &Bound<'_, PyModule>) -> PyResult<()> {
    i8::register_i8(m)?;
    i16::register_i16(m)?;
    i32::register_i32(m)?;
    i64::register_i64(m)?;
    i128::register_i128(m)?;
    u8::register_u8(m)?;
    u16::register_u16(m)?;
    u32::register_u32(m)?;
    u64::register_u64(m)?;
    u128::register_u128(m)?;
    isize::register_isize(m)?;
    usize::register_usize(m)?;
    bool::register_bool(m)?;
    char::register_char(m)?;
    f32::register_f32(m)?;
    f64::register_f64(m)?;
    Ok(())
}
