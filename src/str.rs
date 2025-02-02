use pyo3::exceptions::{PyIndexError, PyKeyError, PyTypeError, PyValueError};
use pyo3::pyclass::CompareOp;
use pyo3::types::{PyDict, PyInt, PySlice, PyString, PyTuple};
use pyo3::{prelude::*, PyResult};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use unicode_general_category::{get_general_category, GeneralCategory};

#[derive(Debug)]
enum Element {
    Literal(String),
    Placeholder(String),
}

fn parse_format_string(s: &str) -> PyResult<Vec<Element>> {
    let mut elements = Vec::new();
    let mut current_literal = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if let Some(&next_c) = chars.peek() {
                    if next_c == '{' {
                        current_literal.push('{');
                        chars.next();
                    } else {
                        if !current_literal.is_empty() {
                            elements.push(Element::Literal(current_literal));
                            current_literal = String::new();
                        }
                        let mut key = String::new();
                        let mut found_closing = false;
                        while let Some(c) = chars.next() {
                            match c {
                                '}' => {
                                    found_closing = true;
                                    break;
                                }
                                '{' => {
                                    return Err(PyValueError::new_err(
                                        "Unexpected '{' inside placeholder",
                                    ));
                                }
                                _ => key.push(c),
                            }
                        }
                        if !found_closing {
                            return Err(PyValueError::new_err("Unclosed placeholder"));
                        }
                        elements.push(Element::Placeholder(key));
                    }
                } else {
                    return Err(PyValueError::new_err("Unclosed placeholder"));
                }
            }
            '}' => {
                if let Some(&next_c) = chars.peek() {
                    if next_c == '}' {
                        current_literal.push('}');
                        chars.next();
                    } else {
                        return Err(PyValueError::new_err("Single '}' in format string"));
                    }
                } else {
                    return Err(PyValueError::new_err("Single '}' in format string"));
                }
            }
            _ => current_literal.push(c),
        }
    }

    if !current_literal.is_empty() {
        elements.push(Element::Literal(current_literal));
    }

    Ok(elements)
}

#[derive(Debug)]
#[pyclass(name = "Str")]
struct Str {
    value: String,
}

#[pymethods]
impl Str {
    #[new]
    #[pyo3(signature = (object = None, encoding = None, errors = None))]
    fn new(
        object: Option<&Bound<'_, PyAny>>,
        encoding: Option<&Bound<'_, PyAny>>,
        errors: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        // If no object is provided, return an empty string.
        if object.is_none() {
            return Ok(Self {
                value: String::new(),
            });
        }
        let obj = object.unwrap();

        // If either encoding or errors is provided, interpret obj as bytes and decode.
        if encoding.is_some() || errors.is_some() {
            // Extract encoding (default "utf-8") and errors (default "strict")
            let encoding: &str = if let Some(enc) = encoding {
                enc.extract()?
            } else {
                "utf-8"
            };
            let errors: &str = if let Some(err) = errors {
                err.extract()?
            } else {
                "strict"
            };

            // For now, only UTF-8 is supported.
            if encoding.to_lowercase() != "utf-8" {
                return Err(PyValueError::new_err("only utf-8 encoding is supported"));
            }

            // Ensure the object is bytes-like.
            if let Ok(py_bytes) = obj.downcast::<pyo3::types::PyBytes>() {
                let bytes = py_bytes.as_bytes();
                match errors {
                    "strict" => match std::str::from_utf8(bytes) {
                        Ok(s) => Ok(Self {
                            value: s.to_owned(),
                        }),
                        Err(e) => Err(PyValueError::new_err(e.to_string())),
                    },
                    "replace" => {
                        // Replace invalid sequences with the Unicode replacement character.
                        let s = String::from_utf8_lossy(bytes).to_string();
                        Ok(Self { value: s })
                    }
                    "ignore" => {
                        // Decode using lossless conversion then filter out replacement chars.
                        let s = String::from_utf8_lossy(bytes)
                            .chars()
                            .filter(|&c| c != '\u{FFFD}')
                            .collect();
                        Ok(Self { value: s })
                    }
                    _ => Err(PyValueError::new_err(format!(
                        "unknown error handler '{}'",
                        errors
                    ))),
                }
            } else {
                Err(PyTypeError::new_err(
                    "argument must be a bytes-like object when encoding is specified",
                ))
            }
        } else {
            // No encoding/errors provided: call str(obj) via its __str__ method.
            let py_str = obj.str()?;
            let s = py_str.to_str()?.to_owned();
            Ok(Self { value: s })
        }
    }

    fn __repr__(&self, py: Python<'_>) -> String {
        let py_str = PyString::new(py, &self.value);
        py_str.repr().unwrap().to_str().unwrap().to_owned()
    }

    fn __str__(&self) -> &str {
        &self.value
    }

    fn __hash__(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.value.hash(&mut hasher);
        hasher.finish()
    }

    fn __add__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        let other_str = check_str_type(other)?;
        Ok(Self {
            value: format!("{}{}", self.value, other_str),
        })
    }

    fn __mul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        let count = convert_to_count(other)?;
        Ok(Self {
            value: self.value.repeat(count),
        })
    }

    fn __rmul__(&self, other: &Bound<'_, PyAny>) -> PyResult<Self> {
        self.__mul__(other)
    }

    fn __len__(&self) -> usize {
        self.value.chars().count()
    }

    fn __getitem__(&self, idx: &Bound<'_, PyAny>) -> PyResult<Self> {
        let len_chars = self.value.chars().count();

        if let Ok(slice) = idx.downcast::<PySlice>() {
            let slice_info = slice.indices(len_chars as isize)?;
            let start = slice_info.start;
            let stop = slice_info.stop;
            let step = slice_info.step;
            // let slice_len = slice_info.slicelength;

            if step != 1 {
                return Err(PyValueError::new_err("slice step not implemented"));
            }
            let chars: Vec<char> = self.value.chars().collect();
            let result = chars[start as usize..stop as usize]
                .iter()
                .collect::<String>();
            Ok(Self { value: result })
        } else {
            let idx = convert_to_index(idx, len_chars)?;
            let ch = self
                .value
                .chars()
                .nth(idx)
                .ok_or_else(|| PyIndexError::new_err("string index out of range"))?;
            Ok(Self {
                value: ch.to_string(),
            })
        }
    }

    fn __richcmp__(&self, other: &Bound<'_, PyAny>, op: CompareOp) -> PyResult<bool> {
        let other_str = check_str_type(other)?;
        Ok(match op {
            CompareOp::Eq => self.value == other_str,
            CompareOp::Ne => self.value != other_str,
            CompareOp::Lt => self.value < other_str,
            CompareOp::Le => self.value <= other_str,
            CompareOp::Gt => self.value > other_str,
            CompareOp::Ge => self.value >= other_str,
        })
    }

    fn upper(&self) -> Self {
        Self {
            value: self.value.to_uppercase(),
        }
    }

    fn lower(&self) -> Self {
        Self {
            value: self.value.to_lowercase(),
        }
    }

    #[pyo3(signature = (old, new, count = -1))]
    fn replace(
        &self,
        old: &Bound<'_, PyAny>,
        new: &Bound<'_, PyAny>,
        count: isize,
    ) -> PyResult<Self> {
        let old_str = check_str_type(old)?;
        let new_str = check_str_type(new)?;

        let result = if old_str.is_empty() {
            if new_str.is_empty() {
                self.value.clone()
            } else {
                let chars: Vec<char> = self.value.chars().collect();
                let n = chars.len();
                let max_insertions = if count == -1 { n + 1 } else { count as usize };
                let max_insertions = max_insertions.min(n + 1);

                let mut result = String::new();
                for i in 0..=n {
                    if i < max_insertions {
                        result.push_str(&new_str);
                    }
                    if i < n {
                        result.push(chars[i]);
                    }
                }
                result
            }
        } else {
            let mut result = String::new();
            let mut current = 0;
            let old_len = old_str.len();
            let mut remaining = if count == -1 {
                usize::MAX
            } else {
                count as usize
            };

            while remaining > 0 {
                if let Some(pos) = self.value[current..].find(&old_str) {
                    let start = current + pos;
                    let end = start + old_len;
                    result.push_str(&self.value[current..start]);
                    result.push_str(&new_str);
                    current = end;
                    remaining -= 1;
                } else {
                    break;
                }
            }
            result.push_str(&self.value[current..]);
            result
        };

        Ok(Self { value: result })
    }

    fn join(&self, iterable: &Bound<'_, PyAny>) -> PyResult<Self> {
        let elements = iterable
            .try_iter()?
            .map(|item| Ok(check_str_type(&item?)?))
            .collect::<PyResult<Vec<String>>>()?;
        Ok(Self {
            value: elements.join(&self.value),
        })
    }

    fn capitalize(&self) -> Self {
        if self.value.is_empty() {
            return Self {
                value: String::new(),
            };
        }

        let mut chars = self.value.chars();
        let first = chars.next().unwrap();
        let rest: String = chars.collect::<String>().to_lowercase();
        let mut result = String::new();
        result.extend(first.to_uppercase());
        result.push_str(&rest);

        Self { value: result }
    }

    fn casefold(&self) -> Self {
        Self {
            value: caseless::default_case_fold_str(&self.value),
        }
    }

    #[pyo3(signature = (width, fillchar = ' '))]
    fn center(&self, width: isize, fillchar: char) -> Self {
        let current_len = self.value.chars().count() as isize;
        if width <= current_len {
            return Self {
                value: self.value.clone(),
            };
        }
        let total_padding = width - current_len;
        let left_pad = total_padding / 2;
        let right_pad = total_padding - left_pad;
        let left = fillchar.to_string().repeat(left_pad as usize);
        let right = fillchar.to_string().repeat(right_pad as usize);
        Self {
            value: format!("{}{}{}", left, self.value, right),
        }
    }

    #[pyo3(signature = (sub, start=None, end=None))]
    fn count(
        &self,
        sub: &Bound<'_, PyAny>,
        start: Option<&Bound<'_, PyAny>>,
        end: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let sub_str = check_str_type(sub)?;
        let len_chars = self.value.chars().count();

        let adjusted_start = adjust_start(start, len_chars)?;
        let adjusted_end = adjust_end(end, len_chars)?;

        if sub_str.is_empty() {
            if adjusted_start > adjusted_end {
                return Ok(0);
            }
            let slice_len = adjusted_end - adjusted_start;
            return Ok(slice_len + 1);
        }

        let chars: Vec<char> = self.value.chars().collect();
        let slice_chars = &chars[adjusted_start..adjusted_end];
        let slice_str: String = slice_chars.iter().collect();

        Ok(slice_str.matches(&sub_str).count())
    }

    fn encode(&self, encoding: &str, errors: &str) -> PyResult<Vec<u8>> {
        if encoding.to_lowercase() != "utf-8" {
            return Err(PyValueError::new_err("Only UTF-8 encoding is supported"));
        }

        match errors {
            "strict" | "ignore" | "replace" => Ok(self.value.as_bytes().to_vec()),
            _ => Err(PyValueError::new_err(format!(
                "unknown error handler '{}' (available handlers: strict, ignore, replace)",
                errors
            ))),
        }
    }

    #[pyo3(signature = (suffix, start=None, end=None))]
    fn endswith(
        &self,
        suffix: &Bound<'_, PyAny>,
        start: Option<&Bound<'_, PyAny>>,
        end: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let suffix_str = check_str_type(suffix)?;

        // Extract start and end as isize or handle type errors
        let start_opt = match start {
            Some(s) => {
                if let Ok(py_int) = s.downcast::<PyInt>() {
                    Some(py_int.extract::<isize>()?)
                } else {
                    let type_name = s.get_type().name()?;
                    return Err(PyTypeError::new_err(format!(
                        "start must be an integer, not {}",
                        type_name
                    )));
                }
            }
            None => None,
        };
        let end_opt = match end {
            Some(e) => {
                if let Ok(py_int) = e.downcast::<PyInt>() {
                    Some(py_int.extract::<isize>()?)
                } else {
                    let type_name = e.get_type().name()?;
                    return Err(PyTypeError::new_err(format!(
                        "end must be an integer, not {}",
                        type_name
                    )));
                }
            }
            None => None,
        };

        let len_chars = self.value.chars().count();
        let (start, end) = adjust_indices(start_opt, end_opt, len_chars)?;

        let substring: String = self
            .value
            .chars()
            .skip(start)
            .take(end.saturating_sub(start))
            .collect();

        Ok(substring.ends_with(&suffix_str))
    }

    #[pyo3(signature = (tabsize = 8))]
    fn expandtabs(&self, tabsize: isize) -> PyResult<Self> {
        if tabsize < 0 {
            return Err(PyValueError::new_err("tabsize must be >= 0"));
        }
        let tabsize = tabsize as usize;

        let mut result = String::new();
        let mut current_column = 0;

        for c in self.value.chars() {
            match c {
                '\t' => {
                    if tabsize != 0 {
                        let spaces = tabsize - (current_column % tabsize);
                        result.push_str(&" ".repeat(spaces));
                        current_column += spaces;
                    }
                    // tabsize 0: skip the tab, no change to current_column
                }
                '\n' | '\r' => {
                    result.push(c);
                    current_column = 0;
                }
                _ => {
                    result.push(c);
                    current_column += 1;
                }
            }
        }

        Ok(Self { value: result })
    }

    #[pyo3(signature = (sub, start=None, end=None))]
    fn find(
        &self,
        sub: &Bound<'_, PyAny>,
        start: Option<isize>,
        end: Option<isize>,
    ) -> PyResult<isize> {
        let sub_str = check_str_type(sub)?;
        let chars: Vec<char> = self.value.chars().collect();
        let len_chars = chars.len() as isize;

        // Adjust start index
        let adjusted_start = {
            let start_val = start.unwrap_or(0);
            let adjusted = if start_val < 0 {
                start_val + len_chars
            } else {
                start_val
            };
            adjusted.clamp(0, len_chars)
        };

        // Adjust end index
        let adjusted_end = {
            let end_val = end.unwrap_or(len_chars);
            let adjusted = if end_val < 0 {
                end_val + len_chars
            } else {
                end_val
            };
            adjusted.clamp(0, len_chars)
        };

        // Handle empty substring case
        if sub_str.is_empty() {
            return Ok(if adjusted_start <= adjusted_end {
                adjusted_start
            } else {
                -1
            });
        }

        let sub_chars: Vec<char> = sub_str.chars().collect();
        let sub_len = sub_chars.len() as isize;

        // Check if substring is longer than the search range
        if (adjusted_end - adjusted_start) < sub_len {
            return Ok(-1);
        }

        // Iterate through possible starting positions
        for i in adjusted_start..=(adjusted_end - sub_len) {
            let i_usize = i as usize;
            let end_idx = i_usize + sub_len as usize;
            let current_slice = &chars[i_usize..end_idx];
            if current_slice == sub_chars.as_slice() {
                return Ok(i);
            }
        }

        Ok(-1)
    }

    fn format(
        &self,
        py: Python<'_>,
        args: &Bound<'_, PyTuple>,
        kwargs: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Self> {
        let elements = parse_format_string(&self.value)?;
        let mut result = String::new();
        let mut current_pos = 0;

        for element in elements {
            match element {
                Element::Literal(s) => result.push_str(&s),
                Element::Placeholder(key) => {
                    let arg = if key.is_empty() {
                        if current_pos >= args.len() {
                            return Err(PyIndexError::new_err(
                                "Replacement index out of range for positional args",
                            ));
                        }
                        let obj = args.get_item(current_pos)?;
                        current_pos += 1;
                        obj
                    } else {
                        let kwargs = kwargs.ok_or_else(|| {
                            PyKeyError::new_err(format!("Keyword argument '{}' not found", key))
                        })?;
                        kwargs.get_item(key.clone())?.ok_or_else(|| {
                            PyKeyError::new_err(format!("Keyword argument '{}' not found", key))
                        })?
                    };

                    let s = arg.to_string();
                    result.push_str(&s);
                }
            }
        }

        Ok(Self { value: result })
    }

    fn format_map(&self, py: Python<'_>, mapping: &Bound<'_, PyAny>) -> PyResult<Self> {
        let elements = parse_format_string(&self.value)?;
        let mut result = String::new();
        let mut current_pos = 0;

        for element in elements {
            match element {
                Element::Literal(s) => result.push_str(&s),
                Element::Placeholder(key) => {
                    let lookup_key = if key.is_empty() {
                        let key_str = current_pos.to_string();
                        current_pos += 1;
                        key_str
                    } else {
                        key
                    };

                    // Directly use get_item()? to propagate KeyError
                    let arg = mapping.get_item(&lookup_key)?;
                    let s = arg.to_string();
                    result.push_str(&s);
                }
            }
        }

        Ok(Self { value: result })
    }

    #[pyo3(signature = (sub, start=None, end=None))]
    fn index(
        &self,
        sub: &Bound<'_, PyAny>,
        start: Option<&Bound<'_, PyAny>>,
        end: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<usize> {
        let sub_str = check_str_type(sub)?;
        let len_chars = self.value.chars().count();

        // Process start parameter
        let start_idx = if let Some(s) = start {
            if let Ok(py_int) = s.downcast::<PyInt>() {
                let idx = py_int.extract::<isize>()?;
                let adjusted = if idx < 0 {
                    idx + len_chars as isize
                } else {
                    idx
                };
                adjusted
            } else {
                let type_name = s.get_type().name()?;
                return Err(PyTypeError::new_err(format!(
                    "start must be an integer, not {}",
                    type_name
                )));
            }
        } else {
            0
        };
        let start_idx = start_idx.clamp(0, len_chars as isize) as usize;

        // Process end parameter
        let end_idx = if let Some(e) = end {
            if let Ok(py_int) = e.downcast::<PyInt>() {
                let idx = py_int.extract::<isize>()?;
                let adjusted = if idx < 0 {
                    idx + len_chars as isize
                } else {
                    idx
                };
                adjusted
            } else {
                let type_name = e.get_type().name()?;
                return Err(PyTypeError::new_err(format!(
                    "end must be an integer, not {}",
                    type_name
                )));
            }
        } else {
            len_chars as isize
        };
        let end_idx = end_idx.clamp(start_idx as isize, len_chars as isize) as usize;

        // Handle empty substring
        if sub_str.is_empty() {
            if start_idx <= end_idx {
                return Ok(start_idx);
            } else {
                return Err(PyValueError::new_err("substring not found"));
            }
        }

        // Convert the original string and sub_str to Vec<char>
        let chars: Vec<char> = self.value.chars().collect();
        let sub_chars: Vec<char> = sub_str.chars().collect();

        // The slice to search in
        let search_slice = &chars[start_idx..end_idx];

        // Check if sub_chars is longer than search_slice
        if sub_chars.len() > search_slice.len() {
            return Err(PyValueError::new_err("substring not found"));
        }

        // Iterate through possible starting positions
        let mut found_pos = None;
        for i in 0..=search_slice.len().saturating_sub(sub_chars.len()) {
            let end_pos = i + sub_chars.len();
            if end_pos > search_slice.len() {
                break;
            }
            if search_slice[i..end_pos] == sub_chars[..] {
                found_pos = Some(i);
                break;
            }
        }

        if let Some(pos) = found_pos {
            Ok(start_idx + pos)
        } else {
            Err(PyValueError::new_err("substring not found"))
        }
    }

    fn isalnum(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value
            .chars()
            .all(|c| c.is_alphabetic() || c.is_numeric())
    }

    fn isalpha(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value.chars().all(|c| c.is_alphabetic())
    }

    fn isascii(&self) -> bool {
        self.value.chars().all(|c| c.is_ascii())
    }

    fn isdecimal(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value
            .chars()
            .all(|c| get_general_category(c) == GeneralCategory::DecimalNumber)
    }

    fn isdigit(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }
        self.value.chars().all(|c| is_numeric_digit(c))
    }

    fn islower(&self) -> bool {
        let mut has_cased = false;
        for c in self.value.chars() {
            let is_cased = c.is_uppercase()
                || c.is_lowercase()
                || get_general_category(c) == GeneralCategory::TitlecaseLetter;

            if is_cased {
                has_cased = true;
                if !c.is_lowercase() {
                    return false;
                }
            }
        }
        has_cased
    }

    fn isidentifier(&self) -> bool {
        let s = &self.value;
        if s.is_empty() {
            return false;
        }
        let mut chars = s.chars();
        let first = chars.next().unwrap();
        if !(first == '_' || first.is_alphabetic()) {
            return false;
        }
        for c in chars {
            if !(c == '_' || c.is_alphanumeric()) {
                return false;
            }
        }
        true
    }

    fn isnumeric(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }

        self.value.chars().all(|c| {
            let category = get_general_category(c);
            matches!(
                category,
                GeneralCategory::DecimalNumber |    // Nd
                GeneralCategory::LetterNumber |     // Nl
                GeneralCategory::OtherNumber // No
            )
        })
    }

    fn isprintable(&self) -> bool {
        // Empty string is considered printable
        if self.value.is_empty() {
            return true;
        }

        self.value.chars().all(|c| {
            !matches!(
                get_general_category(c),
                GeneralCategory::Control       // Cc (control characters)
                | GeneralCategory::Format      // Cf (format characters)
                | GeneralCategory::Surrogate   // Cs (surrogate code points)
                | GeneralCategory::PrivateUse  // Co (private use)
                | GeneralCategory::Unassigned // Cn (unassigned)
            )
        })
    }

    fn isspace(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }

        self.value.chars().all(|c| {
            // Check ASCII whitespace characters first
            matches!(c, '\t' | '\n' | '\x0B' | '\x0C' | '\r') ||
            // Check Unicode whitespace categories
            matches!(
                get_general_category(c),
                GeneralCategory::SpaceSeparator |  // Zs
                GeneralCategory::LineSeparator |   // Zl
                GeneralCategory::ParagraphSeparator  // Zp
            )
        })
    }

    fn istitle(&self) -> bool {
        if self.value.is_empty() {
            return false;
        }

        let mut has_cased = false;
        let mut require_upper_next = true;

        for c in self.value.chars() {
            let is_cased = c.is_uppercase()
                || c.is_lowercase()
                || get_general_category(c) == GeneralCategory::TitlecaseLetter;

            if is_cased {
                has_cased = true;

                if require_upper_next {
                    // Check for uppercase or titlecase
                    let is_titlecase = get_general_category(c) == GeneralCategory::TitlecaseLetter;
                    if !(c.is_uppercase() || is_titlecase) {
                        return false;
                    }
                    require_upper_next = false;
                } else {
                    // Must be lowercase
                    if !c.is_lowercase() {
                        return false;
                    }
                }
            } else {
                // Reset word boundary for non-cased characters
                if !require_upper_next {
                    require_upper_next = true;
                }
            }
        }

        has_cased
    }

    fn isupper(&self) -> bool {
        let mut has_cased = false;
        for c in self.value.chars() {
            let is_cased = c.is_uppercase()
                || c.is_lowercase()
                || get_general_category(c) == GeneralCategory::TitlecaseLetter;

            if is_cased {
                has_cased = true;
                if !c.is_uppercase() {
                    return false;
                }
            }
        }
        has_cased
    }

    // #[pyo3(signature = (width, fillchar = " "))]
    fn ljust(&self, width: usize, fillchar: &Bound<'_, PyAny>) -> PyResult<Self> {
        let fill_str = check_str_type(fillchar)?;
        let mut chars = fill_str.chars();
        let fill_char = chars
            .next()
            .ok_or_else(|| PyTypeError::new_err("fillchar must be a single character"))?;
        if chars.next().is_some() {
            return Err(PyTypeError::new_err("fillchar must be a single character"));
        }

        let current_length = self.value.chars().count();
        if width <= current_length {
            Ok(Self {
                value: self.value.clone(),
            })
        } else {
            let padding = width - current_length;
            let mut new_value = self.value.clone();
            new_value.extend(std::iter::repeat(fill_char).take(padding));
            Ok(Self { value: new_value })
        }
    }

    fn lstrip(&self, chars: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let stripped_value = match chars {
            None => self.value.trim_start().to_string(),
            Some(chars_bound) => {
                let chars_str = check_str_type(chars_bound)?;
                if chars_str.is_empty() {
                    self.value.clone()
                } else {
                    let chars_set: HashSet<char> = chars_str.chars().collect();
                    let leading = self
                        .value
                        .chars()
                        .take_while(|c| chars_set.contains(c))
                        .count();
                    self.value.chars().skip(leading).collect()
                }
            }
        };
        Ok(Self {
            value: stripped_value,
        })
    }

    #[staticmethod]
    fn maketrans(
        x: &Bound<'_, PyAny>,
        y: Option<&Bound<'_, PyAny>>,
        z: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyDict>> {
        let py = x.py();

        // Handle dictionary case (x is a dict, y and z must be None)
        if let Ok(dict) = x.downcast::<PyDict>() {
            if y.is_some() || z.is_some() {
                return Err(PyTypeError::new_err(
                    "when x is a dict, y and z must be None",
                ));
            }

            let output_dict = PyDict::new_bound(py);
            for (key_py, value_py) in dict.iter() {
                // Process key
                let key_codepoint = if let Ok(s) = key_py.downcast::<PyString>() {
                    let s_str = s.to_str()?;
                    if s_str.chars().count() != 1 {
                        return Err(PyValueError::new_err(
                            "string keys in translate table must be of length 1",
                        ));
                    }
                    s_str.chars().next().unwrap() as u32
                } else if let Ok(i) = key_py.downcast::<PyInt>() {
                    let code = i.extract::<i64>()?;
                    if code < 0 || code > 0x10FFFF as i64 {
                        return Err(PyValueError::new_err("character code out of range"));
                    }
                    code as u32
                } else {
                    return Err(PyTypeError::new_err(
                        "keys must be integers or strings of length 1",
                    ));
                };

                // Process value
                let value_obj = if value_py.is_none() {
                    py.None().into()
                } else if let Ok(s) = value_py.downcast::<PyString>() {
                    s.to_object(py)
                } else if let Ok(i) = value_py.downcast::<PyInt>() {
                    let code = i.extract::<i64>()?;
                    if code < 0 || code > 0x10FFFF as i64 {
                        return Err(PyValueError::new_err("character code out of range"));
                    }
                    i.to_object(py)
                } else {
                    return Err(PyTypeError::new_err(
                        "values must be integers, strings, or None",
                    ));
                };

                output_dict.set_item(key_codepoint, value_obj)?;
            }

            Ok(output_dict.into())
        } else {
            // Handle string case (x and y are strings, z is optional string)
            let y = y.ok_or_else(|| {
                PyTypeError::new_err("maketrans() argument 2 must be str, not None")
            })?;

            let x_str = x.downcast::<PyString>()?.to_str()?;
            let y_str = y.downcast::<PyString>()?.to_str()?;

            if x_str.chars().count() != y_str.chars().count() {
                return Err(PyValueError::new_err(
                    "the first two maketrans arguments must have equal length",
                ));
            }

            let output_dict = PyDict::new_bound(py);
            for (a, b) in x_str.chars().zip(y_str.chars()) {
                output_dict.set_item(a as u32, b as u32)?;
            }

            // Process z if present
            if let Some(z) = z {
                let z_str = z.downcast::<PyString>()?.to_str()?;
                for c in z_str.chars() {
                    output_dict.set_item(c as u32, py.None())?;
                }
            }

            Ok(output_dict.into())
        }
    }

    fn partition(
        &self,
        sep: &Bound<'_, PyAny>,
        py: Python<'_>,
    ) -> PyResult<(Py<Self>, Py<Self>, Py<Self>)> {
        let sep_str = check_str_type(sep)?;
        if sep_str.is_empty() {
            return Err(PyValueError::new_err("empty separator"));
        }

        if let Some(index) = self.value.find(&sep_str) {
            let end = index + sep_str.len();
            let left = &self.value[..index];
            let sep_part = &self.value[index..end];
            let right = &self.value[end..];
            Ok((
                Py::new(
                    py,
                    Str {
                        value: left.to_string(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: sep_part.to_string(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: right.to_string(),
                    },
                )?,
            ))
        } else {
            Ok((
                Py::new(
                    py,
                    Str {
                        value: self.value.clone(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: String::new(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: String::new(),
                    },
                )?,
            ))
        }
    }

    fn removeprefix(&self, prefix: &Bound<'_, PyAny>) -> PyResult<Self> {
        let prefix_str = check_str_type(prefix)?;

        Ok(Self {
            value: if self.value.starts_with(&prefix_str) {
                self.value[prefix_str.len()..].to_string()
            } else {
                self.value.clone()
            },
        })
    }

    fn removesuffix(&self, suffix: &Bound<'_, PyAny>) -> PyResult<Self> {
        let suffix_str = check_str_type(suffix)?;

        Ok(Self {
            value: if self.value.ends_with(&suffix_str) {
                let end = self.value.len() - suffix_str.len();
                self.value[..end].to_string()
            } else {
                self.value.clone()
            },
        })
    }

    fn rfind(
        &self,
        sub: &Bound<'_, PyAny>,
        start: Option<isize>,
        end: Option<isize>,
    ) -> PyResult<isize> {
        let sub_str = check_str_type(sub)?;
        let len_chars = self.value.chars().count();

        // Adjust start and end indices according to Python's slicing rules
        let adj_start = match start {
            Some(s) => {
                let s = if s < 0 {
                    (len_chars as isize).saturating_add(s)
                } else {
                    s
                };
                s.clamp(0, len_chars as isize) as usize
            }
            None => 0,
        };

        let adj_end = match end {
            Some(e) => {
                let e = if e < 0 {
                    (len_chars as isize).saturating_add(e)
                } else {
                    e
                };
                e.clamp(adj_start as isize, len_chars as isize) as usize
            }
            None => len_chars,
        };

        // Handle empty substring case
        if sub_str.is_empty() {
            return Ok(if adj_start < adj_end {
                adj_end as isize
            } else {
                -1
            });
        }

        // Convert character indices to byte indices
        let (byte_start, byte_end) = self.char_range_to_byte_range(adj_start, adj_end);
        let slice = &self.value[byte_start..byte_end];

        // Find the last occurrence of the substring
        if let Some(byte_idx_in_slice) = slice.rfind(&sub_str) {
            let absolute_byte_idx = byte_start + byte_idx_in_slice;
            let char_idx = self.byte_index_to_char_index(absolute_byte_idx);
            Ok(char_idx as isize)
        } else {
            Ok(-1)
        }
    }

    fn rindex(
        &self,
        sub: &Bound<'_, PyAny>,
        start: Option<isize>,
        end: Option<isize>,
    ) -> PyResult<usize> {
        let found = self.rfind(sub, start, end)?;
        if found == -1 {
            Err(PyValueError::new_err("substring not found"))
        } else {
            Ok(found as usize)
        }
    }

    // #[pyo3(signature = (width, fillchar = " "))]
    fn rjust(&self, width: usize, fillchar: &Bound<'_, PyAny>) -> PyResult<Self> {
        let fill_str = check_str_type(fillchar)?;
        if fill_str.chars().count() != 1 {
            return Err(PyTypeError::new_err(
                "The fill character must be exactly one character long",
            ));
        }
        let fill_char = fill_str.chars().next().unwrap();
        let len = self.value.chars().count();
        if width <= len {
            Ok(Self {
                value: self.value.clone(),
            })
        } else {
            let padding = width - len;
            let padding_str: String = std::iter::repeat(fill_char).take(padding).collect();
            Ok(Self {
                value: padding_str + &self.value,
            })
        }
    }

    fn rpartition(
        &self,
        sep: &Bound<'_, PyAny>,
        py: Python<'_>,
    ) -> PyResult<(Py<Self>, Py<Self>, Py<Self>)> {
        let sep_str = check_str_type(sep)?;
        if sep_str.is_empty() {
            return Err(PyValueError::new_err("empty separator"));
        }

        if let Some(index) = self.value.rfind(&sep_str) {
            let end = index + sep_str.len();
            let left = &self.value[..index];
            let sep_part = &self.value[index..end];
            let right = &self.value[end..];
            Ok((
                Py::new(
                    py,
                    Str {
                        value: left.to_string(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: sep_part.to_string(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: right.to_string(),
                    },
                )?,
            ))
        } else {
            Ok((
                Py::new(
                    py,
                    Str {
                        value: String::new(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: String::new(),
                    },
                )?,
                Py::new(
                    py,
                    Str {
                        value: self.value.clone(),
                    },
                )?,
            ))
        }
    }

    #[pyo3(signature = (sep=None, maxsplit=-1))]
    fn rsplit(
        &self,
        sep: Option<&Bound<'_, PyAny>>,
        maxsplit: isize,
        py: Python<'_>,
    ) -> PyResult<Vec<Py<Self>>> {
        match sep {
            Some(sep) => {
                let sep_str = check_str_type(sep)?;
                if sep_str.is_empty() {
                    return Err(PyValueError::new_err("empty separator"));
                }

                let maxsplits = if maxsplit == -1 {
                    usize::MAX
                } else {
                    maxsplit as usize
                };

                let mut remaining = self.value.as_str();
                let mut parts = Vec::new();
                let mut splits_left = maxsplits;

                while splits_left > 0 {
                    if let Some(pos) = remaining.rfind(&sep_str) {
                        let (left, right_with_sep) = remaining.split_at(pos);
                        let (_, right) = right_with_sep.split_at(sep_str.len());
                        parts.push(right);
                        remaining = left;
                        splits_left -= 1;
                    } else {
                        break;
                    }
                }

                parts.push(remaining);
                parts.reverse();

                parts
                    .into_iter()
                    .map(|s| {
                        Py::new(
                            py,
                            Str {
                                value: s.to_string(),
                            },
                        )
                    })
                    .collect()
            }
            None => {
                let trimmed = self.value.trim();
                if trimmed.is_empty() {
                    return Ok(Vec::new());
                }

                let mut split_points = Vec::new();
                let mut in_whitespace = false;

                // Collect split points by whitespace from the end
                let mut chars = trimmed.char_indices().rev().peekable();
                while let Some((i, c)) = chars.next() {
                    if c.is_whitespace() {
                        if !in_whitespace {
                            split_points.push(i);
                            in_whitespace = true;
                        }
                    } else {
                        in_whitespace = false;
                    }
                }

                split_points.reverse(); // Now ordered from start to end of the trimmed string

                let maxsplits = if maxsplit == -1 {
                    split_points.len()
                } else {
                    maxsplit as usize
                };
                let split_count = maxsplits.min(split_points.len());

                let mut parts = Vec::with_capacity(split_count + 1);
                let mut prev = trimmed.len();

                for &split_point in split_points.iter().take(split_count) {
                    let whitespace_end = trimmed[split_point..]
                        .char_indices()
                        .take_while(|(_, c)| c.is_whitespace())
                        .last()
                        .map(|(i, _)| split_point + i + 1)
                        .unwrap_or(split_point + 1);

                    let part = &trimmed[whitespace_end..prev];
                    parts.push(part);
                    prev = split_point;
                }

                parts.push(&trimmed[0..prev]);
                parts.reverse();

                parts
                    .into_iter()
                    .map(|s| {
                        Py::new(
                            py,
                            Str {
                                value: s.to_string(),
                            },
                        )
                    })
                    .collect()
            }
        }
    }

    #[pyo3(signature = (chars=None))]
    fn rstrip(&self, chars: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let new_value = match chars {
            None => self.value.trim_end().to_string(),
            Some(chars_any) => {
                let chars_str = check_str_type(chars_any)?;
                let chars_set: std::collections::HashSet<char> = chars_str.chars().collect();
                let chars_vec: Vec<char> = self.value.chars().collect();
                let mut end = chars_vec.len();
                while end > 0 && chars_set.contains(&chars_vec[end - 1]) {
                    end -= 1;
                }
                chars_vec[0..end].iter().collect()
            }
        };
        Ok(Self { value: new_value })
    }

    #[pyo3(signature = (sep=None, maxsplit=-1))]
    fn split(
        &self,
        py: Python<'_>,
        sep: Option<&Bound<'_, PyAny>>,
        maxsplit: isize,
    ) -> PyResult<Vec<Py<Self>>> {
        if let Some(sep_val) = sep {
            // Handle specified separator case
            let sep_str = check_str_type(sep_val)?;
            if sep_str.is_empty() {
                return Err(PyValueError::new_err("empty separator"));
            }

            let maxsplit = if maxsplit < 0 {
                usize::MAX
            } else {
                maxsplit as usize
            };

            // Unified splitting with splitn
            let count = maxsplit.saturating_add(1);
            let parts = self.value.splitn(count, &sep_str);

            parts
                .map(|s| {
                    Py::new(
                        py,
                        Str {
                            value: s.to_string(),
                        },
                    )
                })
                .collect()
        } else {
            // Original whitespace splitting logic remains unchanged
            let maxsplit = if maxsplit < 0 {
                usize::MAX
            } else {
                maxsplit as usize
            };
            let trimmed = self.value.trim();

            if trimmed.is_empty() {
                return Ok(vec![]);
            }

            let mut result = Vec::new();
            let mut chars = trimmed.chars().peekable();
            let mut current_token = String::new();
            let mut splits = 0;

            while splits < maxsplit {
                current_token.clear();
                let mut found_whitespace = false;

                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() {
                        found_whitespace = true;
                        break;
                    }
                    current_token.push(ch);
                    chars.next();
                }

                if current_token.is_empty() {
                    break;
                }

                result.push(current_token.clone());

                if !found_whitespace {
                    break;
                }

                while let Some(&ch) = chars.peek() {
                    if ch.is_whitespace() {
                        chars.next();
                    } else {
                        break;
                    }
                }

                splits += 1;
            }

            let remaining: String = chars.collect();
            if !remaining.is_empty() {
                result.push(remaining);
            }

            result
                .into_iter()
                .map(|s| Py::new(py, Str { value: s }))
                .collect()
        }
    }

    #[pyo3(signature = (keepends = false))]
    fn splitlines(&self, py: Python<'_>, keepends: bool) -> PyResult<Vec<Py<Self>>> {
        let mut lines = Vec::new();
        if self.value.is_empty() {
            return Ok(lines);
        }

        let mut line_start = 0;
        let mut chars = self.value.char_indices().peekable();

        while let Some((i, c)) = chars.next() {
            if is_line_break(c) {
                let (line_break_bytes, next_pos) = if c == '\r' {
                    // Check if the next character is \n
                    if let Some(&(j, '\n')) = chars.peek() {
                        // Consume the \n
                        chars.next();
                        (j + '\n'.len_utf8() - i, j + '\n'.len_utf8())
                    } else {
                        (c.len_utf8(), i + c.len_utf8())
                    }
                } else {
                    (c.len_utf8(), i + c.len_utf8())
                };

                let line = &self.value[line_start..i];
                let line_with_break = if keepends {
                    &self.value[line_start..i + line_break_bytes]
                } else {
                    line
                };

                lines.push(Py::new(
                    py,
                    Str {
                        value: line_with_break.to_string(),
                    },
                )?);

                line_start = next_pos;
            }
        }

        // Add any remaining characters after the last line break
        if line_start < self.value.len() {
            lines.push(Py::new(
                py,
                Str {
                    value: self.value[line_start..].to_string(),
                },
            )?);
        }

        // Convert each line to Py<Str>
        lines
            .into_iter()
            .map(|s| {
                Py::new(
                    py,
                    Str {
                        value: s.to_string(),
                    },
                )
            })
            .collect()
    }

    // #[pyo3(signature = (prefix, start=None, end=None))]
    fn startswith(
        &self,
        prefix: &Bound<'_, pyo3::PyAny>,
        start: Option<&Bound<'_, pyo3::PyAny>>,
        end: Option<&Bound<'_, pyo3::PyAny>>,
    ) -> PyResult<bool> {
        let len_chars = self.value.chars().count();
        let adjusted_start = adjust_start(start, len_chars)?;
        let adjusted_end = adjust_end(end, len_chars)?;
        let slice: String = self
            .value
            .chars()
            .skip(adjusted_start)
            .take(adjusted_end.saturating_sub(adjusted_start))
            .collect();

        // If 'prefix' is a tuple, check if any element is a matching prefix.
        if let Ok(prefix_tuple) = prefix.downcast::<pyo3::types::PyTuple>() {
            for item in prefix_tuple.iter() {
                let p = check_str_type(&item)?;
                if slice.starts_with(&p) {
                    return Ok(true);
                }
            }
            Ok(false)
        } else {
            let prefix_str = check_str_type(prefix)?;
            Ok(slice.starts_with(&prefix_str))
        }
    }

    #[pyo3(signature = (chars=None))]
    fn strip(&self, chars: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        match chars {
            // Default behavior: strip Unicode whitespace.
            None => Ok(Self {
                value: self.value.trim().to_string(),
            }),
            Some(chars_bound) => {
                let chars_str = check_str_type(chars_bound)?;
                // If the chars string is empty, do nothing.
                if chars_str.is_empty() {
                    return Ok(Self {
                        value: self.value.clone(),
                    });
                }
                // Build a set of characters to strip.
                let chars_set: std::collections::HashSet<char> = chars_str.chars().collect();
                let char_vec: Vec<char> = self.value.chars().collect();
                let mut start = 0;
                let mut end = char_vec.len();
                // Strip from the start.
                while start < end && chars_set.contains(&char_vec[start]) {
                    start += 1;
                }
                // Strip from the end.
                while end > start && chars_set.contains(&char_vec[end - 1]) {
                    end -= 1;
                }
                let new_value: String = char_vec[start..end].iter().collect();
                Ok(Self { value: new_value })
            }
        }
    }

    fn swapcase(&self) -> Self {
        let mut result = String::with_capacity(self.value.len());
        for c in self.value.chars() {
            if c.is_lowercase() {
                result.extend(c.to_uppercase());
            } else if c.is_uppercase()
                || get_general_category(c) == GeneralCategory::TitlecaseLetter
            {
                result.extend(c.to_lowercase());
            } else {
                result.push(c);
            }
        }
        Self { value: result }
    }

    fn title(&self) -> Self {
        let mut result = String::with_capacity(self.value.len());
        let mut new_word = true;
        for c in self.value.chars() {
            if c.is_alphabetic() {
                if new_word {
                    result.extend(c.to_uppercase());
                } else {
                    result.extend(c.to_lowercase());
                }
                new_word = false;
            } else {
                result.push(c);
                new_word = true;
            }
        }
        Self { value: result }
    }
}

fn is_line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r'
            | '\x0B'
            | '\x0C'
            | '\x1C'
            | '\x1D'
            | '\x1E'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}
fn convert_to_string(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    Ok(obj.to_string())
}

fn check_str_type(obj: &Bound<'_, PyAny>) -> PyResult<String> {
    if let Ok(s) = obj.downcast::<Str>() {
        Ok(s.borrow().value.clone())
    } else if let Ok(s) = obj.downcast::<PyString>() {
        Ok(s.to_str()?.to_owned())
    } else {
        let type_name = obj.get_type().name()?;
        Err(PyTypeError::new_err(format!(
            "Expected string, got {}",
            type_name
        )))
    }
}

fn convert_to_count(obj: &Bound<'_, PyAny>) -> PyResult<usize> {
    let count = if let Ok(py_int) = obj.downcast::<PyInt>() {
        py_int.extract::<isize>()?
    } else {
        let type_name = obj.get_type().name()?;
        return Err(PyTypeError::new_err(format!(
            "can't multiply sequence by non-int of type '{}'",
            type_name
        )));
    };
    Ok(if count < 0 { 0 } else { count as usize })
}

fn convert_to_index(idx: &Bound<'_, PyAny>, len: usize) -> PyResult<usize> {
    let index = if let Ok(py_int) = idx.downcast::<PyInt>() {
        py_int.extract::<isize>()?
    } else {
        let type_name = idx.get_type().name()?;
        return Err(PyTypeError::new_err(format!(
            "string indices must be integers, not {}",
            type_name
        )));
    };

    let adjusted = if index < 0 {
        index + len as isize
    } else {
        index
    };

    if adjusted < 0 || adjusted >= len as isize {
        Err(PyIndexError::new_err("string index out of range"))
    } else {
        Ok(adjusted as usize)
    }
}

fn adjust_start(start: Option<&Bound<'_, PyAny>>, len: usize) -> PyResult<usize> {
    adjust_bound(start, len, 0)
}

fn adjust_end(end: Option<&Bound<'_, PyAny>>, len: usize) -> PyResult<usize> {
    adjust_bound(end, len, len)
}

fn adjust_bound(bound: Option<&Bound<'_, PyAny>>, len: usize, default: usize) -> PyResult<usize> {
    let len_isize = len as isize;
    match bound {
        Some(b) => {
            let idx = b.downcast::<PyInt>()?;
            let idx_val = idx.extract::<isize>()?;
            let adjusted = if idx_val < 0 {
                len_isize + idx_val
            } else {
                idx_val
            };
            let adjusted = adjusted.max(0).min(len_isize) as usize;
            Ok(adjusted)
        }
        None => Ok(default),
    }
}

fn adjust_indices(
    start: Option<isize>,
    end: Option<isize>,
    len_chars: usize,
) -> PyResult<(usize, usize)> {
    let len = len_chars as isize;

    let adjusted_start = start.unwrap_or(0);
    let adjusted_start = if adjusted_start < 0 {
        adjusted_start + len
    } else {
        adjusted_start
    };
    let adjusted_start = adjusted_start.max(0).min(len);

    let adjusted_end = end.unwrap_or(len);
    let adjusted_end = if adjusted_end < 0 {
        adjusted_end + len
    } else {
        adjusted_end
    };
    let adjusted_end = adjusted_end.max(0).min(len);

    Ok((adjusted_start as usize, adjusted_end as usize))
}

fn is_numeric_digit(c: char) -> bool {
    // First check if it's a decimal number (Nd category)
    if get_general_category(c) == GeneralCategory::DecimalNumber {
        return true;
    }

    // Then check other digit characters explicitly
    match c {
        // Superscript digits (e.g. ¹, ², ³)
        '\u{00B2}' | '\u{00B3}' | '\u{00B9}' | '\u{2070}'..='\u{2079}' |
        // Circled numbers (e.g. ①, ②, ③)
        '\u{2460}'..='\u{249B}' |
        // Fullwidth digits
        '\u{FF10}'..='\u{FF19}' => true,
        _ => false
    }
}

impl Str {
    fn char_range_to_byte_range(&self, start_char: usize, end_char: usize) -> (usize, usize) {
        let mut byte_start = self.value.len();
        let mut byte_end = self.value.len();
        let mut current_char = 0;

        for (byte_idx, _) in self.value.char_indices() {
            if current_char == start_char {
                byte_start = byte_idx;
            }
            if current_char == end_char {
                byte_end = byte_idx;
                break;
            }
            current_char += 1;
        }

        if end_char >= current_char {
            byte_end = self.value.len();
        }

        (byte_start, byte_end)
    }

    fn byte_index_to_char_index(&self, byte_idx: usize) -> usize {
        let mut char_count = 0;
        for (i, _) in self.value.char_indices() {
            if i >= byte_idx {
                break;
            }
            char_count += 1;
        }
        char_count
    }
}

#[pymodule]
pub fn register_str(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Str>()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_casefold_eszett() {
        let s = Str {
            value: "ß".to_string(),
        };
        assert_eq!(s.casefold().value, "ss");
    }

    #[test]
    fn test_casefold_dotted_i() {
        let s = Str {
            value: "İ".to_string(),
        };
        let folded = s.casefold().value;
        assert_eq!(folded, "i\u{307}"); // 'i' followed by combining dot above
    }

    #[test]
    fn test_casefold_mixed() {
        let s = Str {
            value: "Hello World! İß".to_string(),
        };
        let expected = "hello world! i\u{307}ss";
        assert_eq!(s.casefold().value, expected);
    }

    #[test]
    fn test_format_map_named() {
        Python::with_gil(|py| {
            let mapping = PyDict::new(py);
            mapping.set_item("greeting", "Hello").unwrap();
            mapping.set_item("name", "Alice").unwrap();
            let s = Str {
                value: "{greeting}, {name}!".to_string(),
            };
            let formatted = s.format_map(py, &mapping).unwrap();
            assert_eq!(formatted.value, "Hello, Alice!");
        });
    }

    #[test]
    fn test_format_map_empty_placeholder() {
        Python::with_gil(|py| {
            let mapping = PyDict::new(py);
            mapping.set_item("0", "Hi").unwrap();
            let s = Str {
                value: "{}".to_string(),
            };
            let formatted = s.format_map(py, &mapping).unwrap();
            assert_eq!(formatted.value, "Hi");
        });
    }

    #[test]
    fn test_format_map_missing_key() {
        Python::with_gil(|py| {
            let mapping = PyDict::new(py);
            let s = Str {
                value: "{missing}".to_string(),
            };
            let result = s.format_map(py, &mapping);
            assert!(result.is_err());
            assert!(result.unwrap_err().is_instance_of::<PyKeyError>(py));
        });
    }

    #[test]
    fn test_decimal_digit() {
        let s = Str {
            value: "0123".into(),
        };
        assert!(s.isdecimal());
        assert!(s.isdigit());

        let s = Str {
            value: "①".into()
        }; // Circled digit
        assert!(!s.isdecimal());
        assert!(s.isdigit());
    }

    #[test]
    fn test_lowercase() {
        let s = Str {
            value: "hello".into(),
        };
        assert!(s.islower());

        let s = Str {
            value: "ḩello".into(),
        }; // Titlecase H
        assert!(!s.islower());
    }

    #[test]
    fn test_numeric() {
        let s = Str {
            value: "0123456789".into(),
        };
        assert!(s.isnumeric()); // Decimal numbers

        let s = Str {
            value: "ⅧⅫ".into()
        };
        assert!(s.isnumeric()); // Roman numerals

        let s = Str {
            value: "²½①".into(),
        };
        assert!(s.isnumeric()); // Superscripts, fractions, circled

        let s = Str {
            value: "１２３".into(),
        };
        assert!(s.isnumeric()); // Fullwidth digits

        let s = Str {
            value: "١٢٣".into(),
        };
        assert!(s.isnumeric()); // Arabic-indic digits
    }

    #[test]
    fn test_non_numeric() {
        let s = Str {
            value: "123a".into(),
        };
        assert!(!s.isnumeric());

        let s = Str {
            value: "¼kg".into(),
        };
        assert!(!s.isnumeric());

        let s = Str { value: "".into() };
        assert!(!s.isnumeric());
    }

    #[test]
    fn test_isspace() {
        // Valid cases
        assert!(Str {
            value: "  \t\n".into()
        }
        .isspace());
        assert!(Str {
            value: "\u{00A0}\u{2028}".into()
        }
        .isspace()); // NBSP + line sep

        // Invalid cases
        assert!(!Str { value: "".into() }.isspace());
        assert!(!Str {
            value: "  a ".into()
        }
        .isspace());
        assert!(!Str {
            value: "\u{200B}".into()
        }
        .isspace()); // Zero-width space
    }

    #[test]
    fn test_edge_cases() {
        assert!(Str {
            value: "\u{3000}".into()
        }
        .isspace()); // Ideographic space (Zs)
        assert!(Str {
            value: "\u{2029}".into()
        }
        .isspace()); // Paragraph separator (Zp)
        assert!(!Str {
            value: "\u{0010}".into()
        }
        .isspace()); // Control character (Cc)
    }

    #[test]
    fn test_title_case() {
        assert!(Str {
            value: "Hello World".into()
        }
        .istitle());
        assert!(Str {
            value: "ǅello World".into()
        }
        .istitle()); // Titlecase first char
        assert!(Str { value: "A".into() }.istitle());
    }

    #[test]
    fn test_non_title_case() {
        assert!(!Str {
            value: "hello World".into()
        }
        .istitle());
        assert!(!Str {
            value: "Hello world".into()
        }
        .istitle());
        assert!(!Str {
            value: "ǅEllo".into()
        }
        .istitle()); // Uppercase after titlecase
        assert!(!Str {
            value: "123".into()
        }
        .istitle()); // No cased characters
        assert!(!Str { value: "".into() }.istitle());
    }

    #[test]
    fn test_upper() {
        let s = Str {
            value: "HELLO".into(),
        };
        assert!(s.isupper());

        let s = Str {
            value: "HELLO123!".into(),
        };
        assert!(s.isupper());
    }

    #[test]
    fn test_non_upper() {
        let s = Str {
            value: "Hello".into(),
        };
        assert!(!s.isupper());

        let s = Str {
            value: "ǅELLO".into(),
        }; // Titlecase first letter
        assert!(!s.isupper());

        let s = Str {
            value: "123".into(),
        };
        assert!(!s.isupper());

        let s = Str { value: "".into() };
        assert!(!s.isupper());
    }

    fn create_str(s: &str) -> Str {
        Str {
            value: s.to_string(),
        }
    }
    fn test_removeprefix_case(input: &str, prefix: &str, expected: &str) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_prefix = PyString::new(py, prefix).into_any();
            let result = s.removeprefix(&py_prefix).unwrap();
            assert_eq!(
                result.value, expected,
                "Input: '{}', Prefix: '{}'",
                input, prefix
            );
        });
    }

    #[test]
    fn test_removeprefix() {
        // Basic cases
        test_removeprefix_case("test_string", "test_", "string");
        test_removeprefix_case("test_string", "nope", "test_string");
        test_removeprefix_case("test_string", "", "test_string");
        test_removeprefix_case("test_string", "test_string", "");

        // Edge cases
        test_removeprefix_case("test_string", "test_string_extra", "test_string");
        test_removeprefix_case("", "anything", "");
        test_removeprefix_case("", "", "");
        test_removeprefix_case("abc", "a", "bc");
        test_removeprefix_case("abc", "abc", "");

        // Multi-byte characters
        test_removeprefix_case("café", "ca", "fé");
        test_removeprefix_case("café", "café", "");
        test_removeprefix_case("🚀rocket", "🚀", "rocket");

        // Case sensitivity
        test_removeprefix_case("Test", "test", "Test");
        test_removeprefix_case("TEST", "test", "TEST");
    }

    fn test_removesuffix_case(input: &str, suffix: &str, expected: &str) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_suffix = PyString::new(py, suffix).into_any();
            let result = s.removesuffix(&py_suffix).unwrap();
            assert_eq!(
                result.value, expected,
                "Input: '{}', Suffix: '{}'",
                input, suffix
            );
        });
    }

    #[test]
    fn test_removesuffix() {
        // Basic functionality
        test_removesuffix_case("test_string", "_string", "test");
        test_removesuffix_case("test_string", "nope", "test_string");
        test_removesuffix_case("test_string", "", "test_string");
        test_removesuffix_case("test_string", "test_string", "");

        // Edge cases
        test_removesuffix_case("test", "est", "t");
        test_removesuffix_case("test", "test", "");
        test_removesuffix_case("short", "short_text", "short");
        test_removesuffix_case("", "anything", "");
        test_removesuffix_case("", "", "");

        // Multi-byte characters
        test_removesuffix_case("café", "fé", "ca");
        test_removesuffix_case("rocket🚀", "🚀", "rocket");
        test_removesuffix_case("日本", "日本", "");

        // Case sensitivity
        test_removesuffix_case("Test", "ST", "Test");
        test_removesuffix_case("TEST", "st", "TEST");
        test_removesuffix_case("Case", "case", "Case");

        // Partial matches
        test_removesuffix_case("abcabc", "abc", "abc");
        test_removesuffix_case("ababa", "aba", "ab");
    }

    fn test_replace_case(input: &str, old: &str, new: &str, count: isize, expected: &str) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_old = PyString::new(py, old).into_any();
            let py_new = PyString::new(py, new).into_any();
            let result = s.replace(&py_old, &py_new, count).unwrap();
            assert_eq!(
                result.value, expected,
                "replace({:?}, {:?}, {:?})",
                input, old, new
            );
        });
    }

    #[test]
    fn test_replace() {
        // Basic functionality
        test_replace_case("aaa", "a", "b", -1, "bbb");
        test_replace_case("aaa", "a", "b", 2, "bba");
        test_replace_case("ababa", "aba", "c", 1, "cba");
        test_replace_case("abcabc", "abc", "x", 1, "xabc");
        test_replace_case("test", "t", "", -1, "es");

        // Empty old cases
        test_replace_case("test", "", "x", 3, "xtxexst");
        test_replace_case("test", "", "x", 0, "test");
        test_replace_case("", "", "x", -1, "x");
        test_replace_case("a", "", "x", 2, "xax");
        test_replace_case("", "", "", -1, "");

        // Count == 0
        test_replace_case("hello", "l", "r", 0, "hello");

        // No match
        test_replace_case("hello", "x", "y", 3, "hello");

        // Edge cases
        test_replace_case("aaaa", "aa", "b", 2, "bb");
        test_replace_case("ababab", "ab", "c", -1, "ccc");
        test_replace_case("abc", "", "x", 2, "xxabc");
        test_replace_case("xyz", "", "-", -1, "-x-y-z-");
    }

    fn test_rfind_case(
        input: &str,
        sub: &str,
        start: Option<isize>,
        end: Option<isize>,
        expected: isize,
    ) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_sub = PyString::new(py, sub).into_any();
            let result = s.rfind(&py_sub, start, end).unwrap();
            assert_eq!(
                result, expected,
                "rfind(input: {:?}, sub: {:?}, start: {:?}, end: {:?})",
                input, sub, start, end
            );
        });
    }

    #[test]
    fn test_rfind() {
        // Basic functionality
        test_rfind_case("abcde", "cd", None, None, 2);
        test_rfind_case("café", "é", Some(3), Some(4), 3);
        test_rfind_case("🚀rocket", "🚀", None, None, 0);
        test_rfind_case("abcabc", "abc", None, None, 3);
        test_rfind_case("abcabc", "abc", Some(1), None, 3);
        test_rfind_case("abcabc", "abc", None, Some(3), 0);

        // Empty substring
        test_rfind_case("abc", "", Some(1), Some(2), 2);
        test_rfind_case("abc", "", Some(2), Some(2), -1);
        test_rfind_case("", "", None, None, 0);
        test_rfind_case("", "", Some(1), None, -1);

        // Not found
        test_rfind_case("hello", "x", None, None, -1);
        test_rfind_case("abc", "abcd", None, None, -1);

        // Edge cases
        test_rfind_case("ababa", "aba", None, None, 2);
        test_rfind_case("ababa", "aba", Some(0), Some(3), 0);
        test_rfind_case("test", "t", None, None, 3);
        test_rfind_case("test", "t", Some(1), None, 3);
        test_rfind_case("test", "t", None, Some(3), 0);

        // Multi-byte characters
        test_rfind_case("café", "fé", None, None, 2);
        test_rfind_case("rocket🚀", "🚀", None, None, 6);
        test_rfind_case("日本", "日本", None, None, 0);
        test_rfind_case("日本語", "語", None, None, 2);

        // Case sensitivity
        test_rfind_case("Test", "st", None, None, 2);
        test_rfind_case("Test", "ST", None, None, -1);
        test_rfind_case("abcde", "de", Some(-3), Some(-1), 3);
    }

    fn test_rindex_case(
        input: &str,
        sub: &str,
        start: Option<isize>,
        end: Option<isize>,
        expected: Option<usize>,
    ) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_sub = PyString::new(py, sub).into_any();
            let result = s.rindex(&py_sub, start, end);
            match expected {
                Some(idx) => assert_eq!(
                    result.unwrap(),
                    idx,
                    "Input: {}, sub: {}, start: {:?}, end: {:?}",
                    input,
                    sub,
                    start,
                    end
                ),
                None => {
                    assert!(
                        result.is_err(),
                        "Expected error for input: {}, sub: {}",
                        input,
                        sub
                    );
                    assert!(result.unwrap_err().is_instance_of::<PyValueError>(py));
                }
            }
        });
    }

    #[test]
    fn test_rindex() {
        // Found cases
        test_rindex_case("abcde", "cd", None, None, Some(2));
        test_rindex_case("abcabc", "abc", None, None, Some(3));
        test_rindex_case("ababa", "aba", None, None, Some(2));
        test_rindex_case("test", "t", None, None, Some(3));
        test_rindex_case("café", "é", Some(3), Some(4), Some(3));
        test_rindex_case("🚀rocket", "🚀", None, None, Some(0));
        test_rindex_case("", "", None, None, Some(0));
        test_rindex_case("abc", "", None, None, Some(3));
        test_rindex_case("abc", "a", Some(-3), Some(3), Some(0));

        // Not found cases
        test_rindex_case("hello", "x", None, None, None);
        test_rindex_case("abc", "abcd", None, None, None);
        test_rindex_case("ababa", "aba", Some(0), Some(3), None);
        test_rindex_case("test", "t", None, Some(3), None);
        test_rindex_case("abc", "", Some(4), None, None);
        test_rindex_case("", "a", None, None, None);
    }

    fn test_rjust_case(input: &str, width: usize, fillchar: &str, expected: &str) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_fillchar = PyString::new(py, fillchar).into_any();
            let result = s.rjust(width, &py_fillchar).unwrap();
            assert_eq!(
                result.value, expected,
                "input: '{}', width: {}, fillchar: '{}'",
                input, width, fillchar
            );
        });
    }

    #[test]
    fn test_rjust() {
        // Default fillchar (space)
        test_rjust_case("test", 4, " ", "test");
        test_rjust_case("test", 5, " ", " test");
        test_rjust_case("test", 6, " ", "  test");
        test_rjust_case("", 3, " ", "   ");

        // Custom fillchar
        test_rjust_case("test", 5, "x", "xtest");
        test_rjust_case("test", 6, "🚀", "🚀test");
        test_rjust_case("a", 3, "b", "bba");
        test_rjust_case("abc", 2, "x", "abc");

        // Multi-byte characters
        test_rjust_case("日本", 3, " ", " 日本");
        test_rjust_case("日", 3, "本", "本本日");
        test_rjust_case("a", 4, "á", "áááa");

        // Edge cases
        test_rjust_case("test", 0, "x", "test");
        test_rjust_case("", 0, "x", "");
    }

    #[test]
    fn test_rjust_invalid_fillchar() {
        Python::with_gil(|py| {
            let s = create_str("test");

            // Empty fillchar
            let py_fillchar_empty = PyString::new(py, "").into_any();
            let result_empty = s.rjust(5, &py_fillchar_empty);
            assert!(result_empty.is_err());
            assert!(result_empty.unwrap_err().is_instance_of::<PyTypeError>(py));

            // Multiple characters
            let py_fillchar_multi = PyString::new(py, "ab").into_any();
            let result_multi = s.rjust(5, &py_fillchar_multi);
            assert!(result_multi.is_err());
            assert!(result_multi.unwrap_err().is_instance_of::<PyTypeError>(py));

            // Non-string fillchar
            let py_fillchar_int = py.eval_bound("123", None, None).unwrap();
            let result_int = s.rjust(5, &py_fillchar_int);
            assert!(result_int.is_err());
            assert!(result_int.unwrap_err().is_instance_of::<PyTypeError>(py));
        });
    }

    fn test_rpartition_case(input: &str, sep: &str, expected: (&str, &str, &str)) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_sep = PyString::new(py, sep).into_any();
            let result = s.rpartition(&py_sep, py).unwrap();
            assert_eq!(result.0.borrow(py).value, expected.0, "Left part mismatch");
            assert_eq!(
                result.1.borrow(py).value,
                expected.1,
                "Separator part mismatch"
            );
            assert_eq!(result.2.borrow(py).value, expected.2, "Right part mismatch");
        });
    }

    #[test]
    fn test_rpartition() {
        // Found cases
        test_rpartition_case("ababa", "aba", ("ab", "aba", ""));
        test_rpartition_case("abcabc", "abc", ("abc", "abc", ""));
        test_rpartition_case("hello", "l", ("hel", "l", "o"));
        test_rpartition_case("test", "t", ("tes", "t", ""));
        test_rpartition_case("café", "fé", ("ca", "fé", ""));
        test_rpartition_case("🚀rocket", "🚀", ("", "🚀", "rocket"));
        test_rpartition_case("a", "a", ("", "a", ""));
        test_rpartition_case("xyz", "xyz", ("", "xyz", ""));

        // Not found cases
        test_rpartition_case("test", "x", ("", "", "test"));
        test_rpartition_case("", "a", ("", "", ""));
        test_rpartition_case("abc", "abcd", ("", "", "abc"));
    }

    #[test]
    fn test_rpartition_empty_sep() {
        Python::with_gil(|py| {
            let s = create_str("test");
            let py_sep = PyString::new(py, "").into_any();
            let result = s.rpartition(&py_sep, py);
            assert!(result.is_err());
            assert!(result.unwrap_err().is_instance_of::<PyValueError>(py));
        });
    }

    fn test_rsplit_case(input: &str, sep: Option<&str>, maxsplit: isize, expected: Vec<&str>) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_sep = sep.map(|s| PyString::new(py, s).into_any());
            let result = s.rsplit(py_sep.as_ref().map(|s| s), maxsplit, py).unwrap();
            let result_strs: Vec<String> =
                result.iter().map(|p| p.borrow(py).value.clone()).collect();
            assert_eq!(
                result_strs, expected,
                "rsplit(input: {:?}, sep: {:?}, maxsplit: {})",
                input, sep, maxsplit
            );
        });
    }

    #[test]
    fn test_rsplit_basic() {
        // No sep (whitespace)
        test_rsplit_case("a b  c   d", None, -1, vec!["a", "b", "c", "d"]);
        test_rsplit_case("a b  c   d", None, 1, vec!["a b  c", "d"]);
        test_rsplit_case("   a b c   ", None, 0, vec!["a b c"]);
        test_rsplit_case("test", None, -1, vec!["test"]);

        // With sep
        test_rsplit_case("a,b,c,d", Some(","), -1, vec!["a", "b", "c", "d"]);
        test_rsplit_case("a,b,c,d", Some(","), 1, vec!["a,b,c", "d"]);
        test_rsplit_case("a=>b=>c=>d", Some("=>"), 2, vec!["a", "b", "c=>d"]);
    }

    #[test]
    fn test_rsplit_edge_cases() {
        // Empty string
        test_rsplit_case("", None, -1, vec![]);
        test_rsplit_case("", Some("x"), -1, vec![""]);

        // Sep not found
        test_rsplit_case("hello", Some("x"), 3, vec!["hello"]);

        // Maxsplit == 0
        test_rsplit_case("a,b,c,d", Some(","), 0, vec!["a,b,c,d"]);

        // Maxsplit larger than possible splits
        test_rsplit_case("a,b,c", Some(","), 5, vec!["a", "b", "c"]);

        // Empty sep (invalid)
        Python::with_gil(|py| {
            let s = create_str("test");
            let py_sep = PyString::new(py, "").into_any();
            let result = s.rsplit(Some(&py_sep), -1, py);
            assert!(result.is_err());
            assert!(result.unwrap_err().is_instance_of::<PyValueError>(py));
        });
    }

    #[test]
    fn test_rsplit_whitespace_edge() {
        // Whitespace only
        test_rsplit_case("   ", None, -1, vec![]);

        // Leading/trailing whitespace
        test_rsplit_case("  a b  ", None, 1, vec!["a", "b"]);
        test_rsplit_case("  a b  ", None, 0, vec!["a b"]);

        // Multi-byte whitespace
        test_rsplit_case("a\u{3000}b\tc", None, -1, vec!["a", "b", "c"]);
    }

    #[test]
    fn test_rsplit_multibyte() {
        test_rsplit_case("🚀a🚀b🚀", Some("🚀"), -1, vec!["", "a", "b", ""]);
        test_rsplit_case("日本|語", Some("|"), 1, vec!["日本", "語"]);
    }

    fn test_rstrip_case(input: &str, chars: Option<&str>, expected: &str) {
        Python::with_gil(|py| {
            let s = create_str(input);
            let py_chars = chars.map(|c| PyString::new(py, c).into_any());
            let result = s.rstrip(py_chars.as_ref().map(|c| c)).unwrap();
            assert_eq!(
                result.value, expected,
                "input: '{}', chars: {:?}",
                input, chars
            );
        });
    }

    #[test]
    fn test_rstrip() {
        // Test None (whitespace)
        test_rstrip_case("  test  ", None, "  test");
        test_rstrip_case("\t\n test \t\n", None, "\t\n test");
        test_rstrip_case("   ", None, "");

        // Test custom chars
        test_rstrip_case("testxx", Some("x"), "test");
        test_rstrip_case("xxxtestxxx", Some("x"), "xxxtest");
        test_rstrip_case("ababa", Some("a"), "abab");
        test_rstrip_case("abcabx", Some("abc"), "abcabx");
        test_rstrip_case("abcabc", Some("abc"), "");
        test_rstrip_case("aabbccbbaa", Some("ab"), "aabbcc");

        // Test empty chars (no stripping)
        test_rstrip_case("test  ", Some(""), "test  ");

        // Test no trailing match
        test_rstrip_case("test", Some("x"), "test");
        test_rstrip_case("", Some("x"), "");

        // Test multi-byte characters
        test_rstrip_case("cafééé", Some("é"), "caf");
        test_rstrip_case("🚀🚀test🚀", Some("🚀"), "🚀🚀test");
        test_rstrip_case("日本語", Some("語"), "日本");
    }
}
