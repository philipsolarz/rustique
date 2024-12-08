use std::borrow::{Borrow, BorrowMut};
use std::ffi::{c_longlong, c_ulonglong};

use num_bigint::BigInt;
use pyo3::exceptions::PyTypeError;
use pyo3::{basic::CompareOp, types::PyFloat};
use pyo3::prelude::*;
use rug::ops::Pow;
use rug::{Float, Integer};
use pyo3::ffi;
use pyo3::types::{
    PyAny, PyInt, PyString

};
use std::time::Instant;
use rayon::prelude::*;
use std::os::raw::{c_void, c_int};

// fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
//     if let Ok(py_int) = obj.downcast::<PyInt>() {
//         unsafe {
//             let py_long_ptr = py_int.as_ptr();

//             // 1. Attempt fast extraction for smaller integers using PyLong_AsLongLong
//             let fast_path = ffi::PyLong_AsLongLong(py_long_ptr);
//             if ffi::PyErr_Occurred().is_null() {
//                 // Successfully extracted as long long
//                 return Ok(Integer::from(fast_path as c_longlong));
//             }

//             // Clear the Python error because PyLong_AsLongLong sets one if it fails
//             ffi::PyErr_Clear();

//             // 2. Attempt unsigned version for smaller positive integers
//             let fast_unsigned = ffi::PyLong_AsUnsignedLongLong(py_long_ptr);
//             if ffi::PyErr_Occurred().is_null() {
//                 return Ok(Integer::from(fast_unsigned as c_ulonglong));
//             }

//             // Clear the Python error again
//             ffi::PyErr_Clear();

//             // 3. Fallback to extracting raw bytes for large integers
//             let num_bits = ffi::_PyLong_NumBits(py_long_ptr);

//             let num_bytes = ((num_bits + 7) / 8) as usize;
//             let mut buffer = vec![0u8; num_bytes];

//             let res = ffi::_PyLong_AsByteArray(
//                 py_long_ptr as *mut _,
//                 buffer.as_mut_ptr(),
//                 num_bytes,
//                 0, // Big-endian
//                 1, // Signed
//             );

//             if res == -1 {
//                 return Err(PyTypeError::new_err("Failed to extract integer bytes"));
//             }

//             // Convert buffer to rug::Integer
//             let result = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);
//             return Ok(result);
//         }
//     }

//     Err(PyTypeError::new_err(
//         "Provided object cannot be converted to rug::Integer. Expected a Python int.",
//     ))
// }

fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
    if let Ok(py_int) = obj.downcast::<PyInt>() {
        unsafe {
            let py_long_ptr = py_int.as_ptr() as *mut ffi::PyLongObject;

            let num_bits = ffi::_PyLong_NumBits(py_long_ptr as *mut _);
            let num_bytes = ((num_bits + 7) / 8) as usize;

            let mut buffer = vec![0u8; num_bytes];

            let res = ffi::_PyLong_AsByteArray(
                py_long_ptr as *mut _,
                buffer.as_mut_ptr(),
                num_bytes,
                0,
                1,
            );

            if res == -1 {
                return Err(PyTypeError::new_err("Failed to extract integer bytes"));
            }

            let result = Integer::from_digits(&buffer, rug::integer::Order::MsfBe);

            // let res = ffi::PyLong_AsNativeBytes(
            //     py_long_ptr as *mut ffi::PyObject,
            //     buffer.as_mut_ptr() as *mut c_void,
            //     num_bytes as isize,
            //     1, // Signed, native byte order
            // );

            // if res == -1 {
            //     return Err(PyTypeError::new_err("Failed to extract integer bytes"));
            // }

            // let result = Integer::from_digits(&buffer, rug::integer::Order::Lsf);

            return Ok(result);
        }
    }
    // If not a PyInt, return an error
    Err(PyTypeError::new_err(
        "Provided object cannot be converted to rug::Integer. Expected a Python int.",
    ))
}

// fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
//     // If the input is already a Python string
//     if let Ok(py_str) = obj.downcast::<PyString>() {
//         unsafe {
//             let py_data = py_str.data()?; // Access raw bytes
//             let bytes = py_data.as_bytes();
//             return Ok(Integer::from_str_radix(std::str::from_utf8(bytes).map_err(|_| {
//                 PyTypeError::new_err("Invalid UTF-8 in input string")
//             })?, 10).map_err(|_| {
//                 PyTypeError::new_err("Failed to parse integer from string")
//             })?);
//         }
//     }

//     // If the input is a Python int, convert it to a string
//     if let Ok(py_int) = obj.downcast::<PyInt>() {
//         let str_obj = py_int.str()?; // Convert PyInt to PyStr
//         let integer_str = str_obj.to_str()?; // Get the UTF-8 string slice
//         return Ok(Integer::from_str_radix(integer_str, 10).map_err(|_| {
//             PyTypeError::new_err("Failed to parse integer from converted string")
//         })?);
//     }

//     // If neither, return an error
//     Err(PyTypeError::new_err(
//         "Provided object cannot be converted to rug::Integer. Expected int or string.",
//     ))
// }


// fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
//     // Downcast to PyStr and parse
//     if let Ok(py_str) = obj.downcast::<PyString>() {
//         let integer_str = py_str.to_str()?; // Get the string representation
//         return Ok(Integer::from_str_radix(integer_str, 10).map_err(|_| {
//             PyTypeError::new_err("Failed to parse string into Integer")
//         })?);
//     }

//     // If it's not already a string, try converting it to a string
//     let str_obj = obj.str()?; // Convert the input to a PyStr
//     let integer_str = str_obj.to_str()?; // Get the string representation
//     Ok(Integer::from_str_radix(integer_str, 10).map_err(|_| {
//         PyTypeError::new_err("Failed to parse string into Integer")
//     })?)
// }

// fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
//     let overall_start = Instant::now(); // Overall timer

//     // Downcast PyAny to PyInt
//     let downcast_start = Instant::now();
//     if let Ok(py_int) = obj.downcast::<PyInt>() {
//         let downcast_duration = downcast_start.elapsed();
//         println!("Downcast to PyInt took: {:?}", downcast_duration);

//         unsafe {
//             // Timer for accessing PyLongObject pointer
//             let pointer_access_start = Instant::now();
//             let py_long_ptr = py_int.as_ptr() as *const PyLongObject;
//             let pointer_access_duration = pointer_access_start.elapsed();
//             println!("Accessing PyLongObject pointer took: {:?}", pointer_access_duration);

//             // Timer for extracting size and sign
//             let size_start = Instant::now();
//             let size = (*py_long_ptr).ob_base.ob_size;
//             let is_negative = size < 0;
//             let size = size.abs() as usize;
//             let size_duration = size_start.elapsed();
//             println!("Extracting size and sign took: {:?}", size_duration);

//             // Timer for accessing digits
//             let digits_access_start = Instant::now();
//             let digits_ptr = (*py_long_ptr).ob_digit.as_ptr();
//             let digits = std::slice::from_raw_parts(digits_ptr, size);
//             let digits_access_duration = digits_access_start.elapsed();
//             println!("Accessing digits took: {:?}", digits_access_duration);

//             // Timer for reconstructing the integer
//             let reconstruction_start = Instant::now();
//             // Load the precomputed powers of 2^30 once
//             let powers = load_powers_of_2_30();

//             // Reconstruct the integer using precomputed powers
//             let mut result = Integer::from(0u8);
//             for (i, &digit) in digits.iter().enumerate() {
//                 if i >= powers.len() {
//                     panic!("Index {} exceeds precomputed powers of 2^30 table!", i);
//                 }
//                 let digit_value = Integer::from(digit as u64) * &powers[i];
//                 result += digit_value;
//             }

//             if is_negative {
//                 result = -result;
//             }
//             let reconstruction_duration = reconstruction_start.elapsed();
//             println!("Reconstructing integer took: {:?}", reconstruction_duration);

//             // Overall time
//             let overall_duration = overall_start.elapsed();
//             println!("Total time: {:?}", overall_duration);

//             return Ok(result);
//         }
//     }

//     // If not a PyInt, return an error
//     let overall_duration = overall_start.elapsed();
//     println!("Total time (failure case): {:?}", overall_duration);
//     Err(PyTypeError::new_err(
//         "Provided object cannot be converted to rug::Integer. Expected a Python int.",
//     ))
// }
// fn wrap(obj: &Bound<'_, PyAny>) -> PyResult<Integer> {
//     // Check if the object is a PyInt (Python int)
//     let overall_start = Instant::now();
//     if let Ok(py_int) = obj.downcast::<PyInt>() {
//         // Timer for direct extraction as i64
//         let extract_start = Instant::now();
//         if let Ok(small_int) = py_int.extract::<i64>() {
//             let duration = extract_start.elapsed();
//             println!("Direct extraction as i64 took: {:?}", duration);
//             let overall_duration = overall_start.elapsed();
//             println!("Overall time: {:?}", overall_duration);
//             return Ok(Integer::from(small_int));
//         }
//         let extract_duration = extract_start.elapsed();
//         println!("Direct extraction as i64 failed, took: {:?}", extract_duration);

//         // Timer for bit_length calculation
//         let bit_length_start = Instant::now();
//         let bit_length: usize = py_int.call_method0("bit_length")?.extract()?;
//         let bit_length_duration = bit_length_start.elapsed();
//         println!("bit_length calculation took: {:?}", bit_length_duration);

//         // Timer for byte_length calculation
//         let byte_length_start = Instant::now();
//         let byte_length = (bit_length + 7) / 8;
//         let byte_length_duration = byte_length_start.elapsed();
//         println!("byte_length calculation took: {:?}", byte_length_duration);

//         // Timer for to_bytes method
//         let to_bytes_start = Instant::now();
//         let big_endian_bytes: Vec<u8> = py_int
//             .call_method1("to_bytes", (byte_length, "big"))?
//             .extract()?;
//         let to_bytes_duration = to_bytes_start.elapsed();
//         println!("to_bytes extraction took: {:?}", to_bytes_duration);

//         // Timer for rug::Integer conversion
//         let rug_start = Instant::now();
//         let result = Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe);
//         let rug_duration = rug_start.elapsed();
//         println!("Conversion to rug::Integer took: {:?}", rug_duration);

//         // Print overall time
//         let overall_duration = overall_start.elapsed();
//         println!("Overall time: {:?}", overall_duration);

//         return Ok(result);
//     }

//     // Check if the object is a PyFloat (Python float)
//     if let Ok(py_float) = obj.downcast::<PyFloat>() {
//         let float_value: f64 = py_float.extract()?; // Extract the float value

//         // Check if the float is equivalent to an integer
//         if float_value.fract() == 0.0 {
//             return Ok(Integer::from(float_value as i64)); // Convert to integer if no fractional part
//         } else {
//             return Err(PyTypeError::new_err(
//                 "Cannot convert float with fractional part to Integer",
//             ));
//         }
//     }
//     let overall_duration = overall_start.elapsed();
//     println!("Overall time (failure case): {:?}", overall_duration);
//     // If the object is neither PyInt nor PyFloat, return an error
//     Err(PyTypeError::new_err(
//         "Provided object cannot be converted to rug::Integer. Expected int or integer-equivalent float.",
//     ))
// }

// #[derive(FromPyObject)]
#[pyclass]
struct int(Integer);

#[pymethods]
impl int {
    #[new]
    fn new(#[pyo3(from_py_with = "wrap")] value: Integer) -> Self {
        int(Integer::from(value))
    }

    fn __repr__(&self) -> String {
        format!("int({})", self.0)
    }

    fn __str__(&self) -> String {
        self.0.to_string()
    }

    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     if let Ok(other_instance) = other.downcast::<Self>() {
    //         Ok(self.0 == other_instance.borrow().0)
    //     } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //         let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //         let byte_length = (bit_length + 7) / 8;
        
    //         let big_endian_bytes: Vec<u8> = other_int
    //             .call_method1("to_bytes", (byte_length, "big"))?
    //             .extract()?;
    //         Ok(self.0 == Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //     } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //         let float_value: f64 = other_float.extract()?;

    //         if float_value.fract() == 0.0 {
    //             Ok(self.0 == Integer::from(float_value as i64))
    //         } else {
    //             Ok(false)
    //         }
    //     } else {
    //         Ok(false)
    //     }
    // }

    // fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
    //     match op {
    //         CompareOp::Eq => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 == other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 == Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 == Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(false)
    //                 }
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Lt => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 < other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 < Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 < Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(false)
    //                 }
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Le => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 <= other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 <= Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 <= Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(false)
    //                 }
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Ne => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 != other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 != Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 != Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(true)
    //                 }
    //             } else {
    //                 Ok(true)
    //             }
    //         },
    //         CompareOp::Ge => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 >= other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 >= Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 >= Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(false)
    //                 }
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Gt => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 > other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.downcast::<PyInt>() {
    //                 let bit_length: usize = other_int.call_method0("bit_length")?.extract()?;
    //                 let byte_length = (bit_length + 7) / 8;
                
    //                 let big_endian_bytes: Vec<u8> = other_int
    //                     .call_method1("to_bytes", (byte_length, "big"))?
    //                     .extract()?;
    //                 Ok(self.0 > Integer::from_digits(&big_endian_bytes, rug::integer::Order::MsfBe))
    //             } else if let Ok(other_float) = other.downcast::<PyFloat>() {
    //                 let float_value: f64 = other_float.extract()?;

    //                 if float_value.fract() == 0.0 {
    //                     Ok(self.0 > Integer::from(float_value as i64))
    //                 } else {
    //                     Ok(false)
    //                 }
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //     }
    // }

    // fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
    //     match op {
    //         CompareOp::Eq => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 == other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 == Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Lt => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 < other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 < Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Le => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 <= other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 <= Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Ne => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 != other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 != Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Ge => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 >= other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 >= Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //         CompareOp::Gt => {
    //             if let Ok(other_instance) = other.downcast::<Self>() {
    //                 Ok(self.0 > other_instance.borrow().0)
    //             } else if let Ok(other_int) = other.extract::<Integer>() {
    //                 Ok(self.0 > Integer::from(other_int))
    //             } else {
    //                 Ok(false)
    //             }
    //         },
    //     }
    // }
    
    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     // Attempt to extract a reference to Self from the other object
    //     if let Ok(other_instance) = other.extract::<Self>() {
    //         // Compare the internal values for equality
    //         Ok(self.0 == other_instance.0)
    //     } else {
    //         // If extraction fails, the objects are not equal
    //         Ok(false)
    //     }
    // }

    // fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
    //     op.matches(self.0.cmp(&other.0))
    // }

    // fn __bool__(&self) -> bool {
    //     self.0 != 0
    // }
}

// #[pyclass]
// struct num(i32);

// #[pymethods]
// impl num {
//     #[new]
//     fn new(value: i32) -> Self {
//         Self(value)
//     }

//     fn __repr__(slf: &Bound<'_, Self>) -> PyResult<String> {
//         let class_name: Bound<'_, PyString> = slf.get_type().qualname()?;
//         Ok(format!("{}({})", class_name, slf.borrow().0))
//     }

//     fn __str__(&self) -> String {
//         self.0.to_string()
//     }

//     fn __hash__(&self) -> u64 {
//         let mut hasher = DefaultHasher::new();
//         self.0.hash(&mut hasher);
//         hasher.finish()
//     }

//     fn __richcmp__(&self, other: &Self, op: CompareOp) -> PyResult<bool> {
//         match op {
//             CompareOp::Lt => Ok(self.0 < other.0),
//             CompareOp::Le => Ok(self.0 <= other.0),
//             CompareOp::Eq => Ok(self.0 == other.0),
//             CompareOp::Ne => Ok(self.0 != other.0),
//             CompareOp::Ge => Ok(self.0 >= other.0),
//             CompareOp::Gt => Ok(self.0 > other.0),
//         }
//     }

//     // fn __richcmp__(&self, other: &Self, op: CompareOp) -> bool {
//     //     op.matches(self.0.cmp(&other.0))
//     // }

//     fn __bool__(&self) -> bool {
//         self.0 != 0
//     }
// }


// #[derive(FromPyObject)]
// #[pyclass]
// struct list {
//     iterable: Vec<PyObject>,
// }


// #[pymethods]
// impl list {
//     #[new]
//     #[pyo3(signature = (iterable=None))]
//     fn new(iterable: Option<Vec<PyObject>>) -> Self {
//         list {
//             iterable: iterable.unwrap_or_default(),
//         }
//     }

    // fn __eq__(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
    //     let a = other.downcast::<list>()?;
    //     // let b = self.__eq__(a).is_ok();
    //     let b = self.iterable.iter().eq(other.iterable.iter());

    //     Ok(true)
        
    // }

    // fn __eq__(&self, other: PyObject) -> bool {
    //     Python::with_gil(|py| {
    //         // Attempt to downcast the other object to a `list`
    //         if let Ok(list_bound) = other.downcast_bound::<list>(py) {
    //             // Unbind to get the owned Py<list>
    //             let list_abc: Py<list> = list_bound.clone().unbind();
    
    //             // Extract the Rust `list` instance
    //             let other_list = list_abc.borrow(py);
    
                
    //             // Compare the `iterable` of both lists
    //             self.iterable == other_list.iterable
    //         } else {
    //             false // Return false if the other object is not a `list`
    //         }
    //     })
    // }
// }
            
            // if let Ok(other) = other.extract::<list>(py) {
            //     return self.iterable == other.iterable;
            // }

            // let class_bound = class.downcast_bound::<Class>(py)?;

            // Alternatively you can get a `PyRefMut` directly
            // let class_ref: PyRefMut<'_, Class> = class.extract(py)?;
            // assert_eq!(class_ref.i, 1);

//             if other.downcast_bound::<list>(py).is_ok() {

//                 return true;
//             }

//             if other.downcast_bound::<PyList>(py).is_ok() {
//                 return true;
//             }

//             if other.downcast_bound::<PySequence>(py).is_ok() {
//                 return true;
//             }

//             false
//         })
//     }
// }


// #[pyclass]
// struct List {
//     elements: Vec<PyObject>,
// }



// #[pymethods]
// impl List {
//     #[new]
//     #[pyo3(signature = (elements=None))]
//     fn new(py: Python, elements: Option<&Bound<PyAny>>) -> PyResult<Self> {
//         let mut vec = Vec::new();

//         if let Some(iterable) = elements {
//             if let Ok(seq) = iterable.downcast::<PySequence>() {
//                 for item in seq.try_iter()? {
//                     vec.push(item?.into_pyobject(py)?.unbind());
//                 }
//             } else {
//                 for item in iterable.try_iter()? {
//                     vec.push(item?.into_pyobject(py)?.unbind());
//                 }
//             }
//         }

//         Ok(List { elements: vec })
//     }
// }
    // /// Appends a new element to the list.
    // fn append(&mut self, element: PyObject) {
    //     self.elements.push(element);
    // }

    // fn __len__(&self) -> usize {
    //     self.elements.len()
    // }


    // fn __getitem__(&self, index: isize, py: Python) -> PyResult<PyObject> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     self.elements.get(idx)
    //         .cloned()
    //         .ok_or_else(|| PyIndexError::new_err("Index out of range"))
    // }

    // fn __setitem__(&mut self, index: isize, value: PyObject) -> PyResult<()> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     if idx < self.elements.len() {
    //         self.elements[idx] = value;
    //         Ok(())
    //     } else {
    //         Err(PyIndexError::new_err("Index out of range"))
    //     }
    // }

    // fn __delitem__(&mut self, index: isize) -> PyResult<()> {
    //     let idx = if index < 0 {
    //         self.elements.len().checked_sub((-index) as usize)
    //     } else {
    //         Some(index as usize)
    //     }.ok_or_else(|| PyIndexError::new_err("Index out of range"))?;

    //     if idx < self.elements.len() {
    //         self.elements.remove(idx);
    //         Ok(())
    //     } else {
    //         Err(PyIndexError::new_err("Index out of range"))
    //     }
    // }

    // fn __contains__(&self, py: Python, element: PyObject) -> PyResult<bool> {
    //     for obj in &self.elements {
    //         if obj.downcast_bound::<PyAny>(py)?.rich_compare(element.downcast_bound::<PyAny>(py)?, CompareOp::Eq)?.is_truthy()? {
    //             return Ok(true);
    //         }
    //     }
    //     Ok(false)
    // }

    // fn __str__(&self, py: Python) -> PyResult<String> {
    //     let mut s = String::from("[");
    //     for (i, obj) in self.elements.iter().enumerate() {
    //         if i > 0 {
    //             s.push_str(", ");
    //         }
    //         let obj_ref = obj.as_ref(py); // Obtain a &PyAny reference
    //         let obj_str = obj_ref.repr()?.to_str()?; // Call repr() and convert to &str
    //         s.push_str(obj_str);
    //     }
    //     s.push(']');
    //     Ok(s)
    // }
    

    // fn __repr__(&self, py: Python) -> PyResult<String> {
    //     Ok(format!("List({})", self.__str__(py)?))
    // }

    /// Concatenates two lists.
    // fn __add__(&self, other: &List) -> List {
    //     let mut new_elements = self.elements.clone();
    //     new_elements.extend_from_slice(&other.elements);
    //     List {
    //         elements: new_elements,
    //     }
    // }

    // fn __mul__(&self, times: isize) -> List {
    //     let mut new_elements = Vec::new();
    //     for _ in 0..times.max(0) {
    //         new_elements.extend(self.elements.iter().cloned());
    //     }
    //     List {
    //         elements: new_elements,
    //     }
    // }
// }



#[pymodule]
fn rustique(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<int>()?;
    // m.add_class::<num>()?;
    // m.add_class::<list>()?;
    Ok(())
}