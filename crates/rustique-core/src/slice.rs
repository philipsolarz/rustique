//! Python index and slice semantics, implemented once and generically over `Vec<T>`.
//!
//! Both native columns and the binding's object storage (`Vec<Py<PyAny>>`) use these
//! functions, so `list` parity lives in exactly one place. Mutating operations return the
//! elements they removed instead of dropping them: the binding drops Python objects only
//! after releasing its lock, because a decref can run arbitrary Python code (`__del__`).

use std::fmt;

/// Errors raised by slice operations; the binding maps them to Python exceptions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceError {
    /// `slice step cannot be zero` (`ValueError`).
    ZeroStep,
    /// Extended-slice assignment with a mismatched length (`ValueError`).
    LengthMismatch { expected: usize, got: usize },
}

impl fmt::Display for SliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SliceError::ZeroStep => f.write_str("slice step cannot be zero"),
            SliceError::LengthMismatch { expected, got } => write!(
                f,
                "attempt to assign sequence of size {got} to extended slice of size {expected}"
            ),
        }
    }
}

impl std::error::Error for SliceError {}

/// A slice already adjusted to a concrete length, exactly like the result of CPython's
/// `PySlice_AdjustIndices` (and Python's `slice.indices(len)` plus the slice length).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceIndices {
    pub start: isize,
    pub stop: isize,
    pub step: isize,
    /// Number of elements the slice selects.
    pub len: usize,
}

impl SliceIndices {
    /// Build from already-adjusted values (e.g. PyO3's `PySlice::indices`).
    pub fn from_adjusted(start: isize, stop: isize, step: isize, len: usize) -> Self {
        SliceIndices { start, stop, step, len }
    }

    /// Normalise `start:stop:step` against `length` with Python semantics
    /// (`PySlice_Unpack` + `PySlice_AdjustIndices`).
    pub fn new(
        start: Option<isize>,
        stop: Option<isize>,
        step: Option<isize>,
        length: usize,
    ) -> Result<Self, SliceError> {
        let step = step.unwrap_or(1);
        if step == 0 {
            return Err(SliceError::ZeroStep);
        }
        // CPython clamps the step so that `-step` cannot overflow.
        let step = step.max(-isize::MAX);
        let length = length as isize;
        let (lower, upper) = if step < 0 { (-1, length - 1) } else { (0, length) };
        let adjust = |value: Option<isize>, default: isize| -> isize {
            match value {
                None => default,
                Some(v) if v < 0 => (v.saturating_add(length)).max(lower),
                Some(v) => v.min(upper),
            }
        };
        let (start, stop) = if step < 0 {
            (adjust(start, upper), adjust(stop, lower))
        } else {
            (adjust(start, lower), adjust(stop, upper))
        };
        let len = if step < 0 {
            if stop < start { ((start - stop - 1) / (-step) + 1) as usize } else { 0 }
        } else if start < stop {
            ((stop - start - 1) / step + 1) as usize
        } else {
            0
        };
        Ok(SliceIndices { start, stop, step, len })
    }

    /// The selected positions, in slice order.
    pub fn positions(&self) -> impl ExactSizeIterator<Item = usize> + Clone + use<> {
        let (start, step) = (self.start, self.step);
        (0..self.len).map(move |k| (start + k as isize * step) as usize)
    }

    /// True for a contiguous forward slice (`step == 1`).
    pub fn is_contiguous(&self) -> bool {
        self.step == 1
    }
}

/// Resolve a possibly negative index against `len` (`IndexError` when `None`).
pub fn normalize_index(index: isize, len: usize) -> Option<usize> {
    let resolved = if index < 0 { index.checked_add(len as isize)? } else { index };
    if resolved >= 0 && (resolved as usize) < len { Some(resolved as usize) } else { None }
}

/// `list.insert` position semantics: negative indices count from the end and the result
/// is clamped into `0..=len`.
pub fn clamp_insert_index(index: isize, len: usize) -> usize {
    if index < 0 {
        (index.saturating_add(len as isize)).max(0) as usize
    } else {
        (index as usize).min(len)
    }
}

/// Copy the selected elements (`v[s]`).
pub fn take_slice<T: Clone>(v: &[T], s: &SliceIndices) -> Vec<T> {
    if s.step == 1 {
        return v[s.start as usize..s.start as usize + s.len].to_vec();
    }
    s.positions().map(|i| v[i].clone()).collect()
}

/// `del v[s]`; returns the removed elements (in ascending position order).
pub fn delete_slice<T>(v: &mut Vec<T>, s: &SliceIndices) -> Vec<T> {
    if s.len == 0 {
        return Vec::new();
    }
    if s.step == 1 || s.step == -1 {
        let lo = if s.step == 1 { s.start as usize } else { (s.start - (s.len as isize - 1)) as usize };
        return v.drain(lo..lo + s.len).collect();
    }
    // Normalise to ascending positions and remove in one compaction pass.
    let (first, step) = if s.step > 0 {
        (s.start as usize, s.step as usize)
    } else {
        ((s.start + (s.len as isize - 1) * s.step) as usize, (-s.step) as usize)
    };
    let last = first + (s.len - 1) * step;
    let mut removed = Vec::with_capacity(s.len);
    let mut kept = Vec::with_capacity(v.len() - s.len);
    for (i, item) in std::mem::take(v).into_iter().enumerate() {
        if i >= first && i <= last && (i - first) % step == 0 {
            removed.push(item);
        } else {
            kept.push(item);
        }
    }
    *v = kept;
    removed
}

/// `v[s] = values`; returns the replaced elements.
///
/// Contiguous slices (`step == 1`) may change the length; extended slices require
/// `values.len() == s.len` (CPython semantics, including for `step == -1`). On a length
/// mismatch `v` is unchanged and the rejected `values` come back with the error: like
/// removed elements, they must not be dropped while the caller holds its lock.
pub fn try_assign_slice<T>(v: &mut Vec<T>, s: &SliceIndices, values: Vec<T>) -> Result<Vec<T>, (SliceError, Vec<T>)> {
    if s.step == 1 {
        let lo = s.start as usize;
        // CPython: an empty slice with stop < start inserts at start.
        let hi = (s.stop.max(s.start) as usize).min(v.len()).max(lo);
        return Ok(v.splice(lo..hi, values).collect());
    }
    if values.len() != s.len {
        return Err((SliceError::LengthMismatch { expected: s.len, got: values.len() }, values));
    }
    let mut removed = Vec::with_capacity(s.len);
    for (pos, value) in s.positions().zip(values) {
        removed.push(std::mem::replace(&mut v[pos], value));
    }
    Ok(removed)
}

/// [`try_assign_slice`] for elements whose drop has no side effects (native values): a
/// rejected `values` is dropped here.
pub fn assign_slice<T>(v: &mut Vec<T>, s: &SliceIndices, values: Vec<T>) -> Result<Vec<T>, SliceError> {
    try_assign_slice(v, s, values).map_err(|(e, _)| e)
}

/// `v * n` for lists (`n <= 0` yields an empty list).
pub fn repeat<T: Clone>(v: &[T], n: isize) -> Vec<T> {
    if n <= 0 || v.is_empty() {
        return Vec::new();
    }
    let n = n as usize;
    let mut out = Vec::with_capacity(v.len().saturating_mul(n));
    for _ in 0..n {
        out.extend_from_slice(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// Reference implementation of Python slice selection on a model list.
    fn model_positions(len: usize, start: Option<isize>, stop: Option<isize>, step: isize) -> Vec<usize> {
        let s = SliceIndices::new(start, stop, Some(step), len).unwrap();
        let mut out = Vec::new();
        let mut i = s.start;
        if step > 0 {
            while i < s.stop {
                out.push(i as usize);
                i += step;
            }
        } else {
            while i > s.stop {
                out.push(i as usize);
                i += step;
            }
        }
        out
    }

    #[test]
    fn python_examples() {
        // len 5: [::-1], [3:1], [-100:100], [1:-1:2]
        let r = SliceIndices::new(None, None, Some(-1), 5).unwrap();
        assert_eq!(r.positions().collect::<Vec<_>>(), vec![4, 3, 2, 1, 0]);
        assert_eq!(SliceIndices::new(Some(3), Some(1), None, 5).unwrap().len, 0);
        let all = SliceIndices::new(Some(-100), Some(100), None, 5).unwrap();
        assert_eq!((all.start, all.stop, all.len), (0, 5, 5));
        let odd = SliceIndices::new(Some(1), Some(-1), Some(2), 5).unwrap();
        assert_eq!(odd.positions().collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(SliceIndices::new(None, None, Some(0), 5), Err(SliceError::ZeroStep));
    }

    #[test]
    fn assign_contiguous_grows_and_inserts() {
        let mut v = vec![1, 2, 3, 4, 5];
        let s = SliceIndices::new(Some(1), Some(3), None, v.len()).unwrap();
        let removed = assign_slice(&mut v, &s, vec![9, 9, 9, 9]).unwrap();
        assert_eq!(removed, vec![2, 3]);
        assert_eq!(v, vec![1, 9, 9, 9, 9, 4, 5]);

        let mut v = vec![1, 2, 3];
        let s = SliceIndices::new(Some(100), None, None, v.len()).unwrap();
        assign_slice(&mut v, &s, vec![999]).unwrap();
        assert_eq!(v, vec![1, 2, 3, 999]);

        let mut v = vec![1, 2, 3, 4, 5];
        let s = SliceIndices::new(Some(3), Some(1), None, v.len()).unwrap();
        assign_slice(&mut v, &s, vec![7]).unwrap();
        assert_eq!(v, vec![1, 2, 3, 7, 4, 5]);
    }

    #[test]
    fn assign_extended_checks_length() {
        let mut v = vec![0, 1, 2, 3, 4, 5];
        let s = SliceIndices::new(None, None, Some(2), v.len()).unwrap();
        assert_eq!(
            assign_slice(&mut v, &s, vec![1, 2]),
            Err(SliceError::LengthMismatch { expected: 3, got: 2 })
        );
        assign_slice(&mut v, &s, vec![10, 12, 14]).unwrap();
        assert_eq!(v, vec![10, 1, 12, 3, 14, 5]);
        // The rejected values are handed back untouched (the binding drops them unlocked).
        let (e, rejected) = try_assign_slice(&mut v, &s, vec![7, 8, 9, 10]).unwrap_err();
        assert_eq!(e, SliceError::LengthMismatch { expected: 3, got: 4 });
        assert_eq!(rejected, vec![7, 8, 9, 10]);
        assert_eq!(v, vec![10, 1, 12, 3, 14, 5]);
        let rev = SliceIndices::new(None, None, Some(-1), v.len()).unwrap();
        assign_slice(&mut v, &rev, vec![1, 2, 3, 4, 5, 6]).unwrap();
        assert_eq!(v, vec![6, 5, 4, 3, 2, 1]);
    }

    #[test]
    fn indices() {
        assert_eq!(normalize_index(-1, 3), Some(2));
        assert_eq!(normalize_index(3, 3), None);
        assert_eq!(normalize_index(-4, 3), None);
        assert_eq!(normalize_index(isize::MIN, 3), None);
        assert_eq!(clamp_insert_index(-100, 3), 0);
        assert_eq!(clamp_insert_index(100, 3), 3);
        assert_eq!(clamp_insert_index(-1, 3), 2);
        assert_eq!(repeat(&[1, 2], 3), vec![1, 2, 1, 2, 1, 2]);
        assert!(repeat(&[1, 2], -1).is_empty());
    }

    proptest! {
        #[test]
        fn take_matches_model(len in 0usize..40, start in proptest::option::of(-50isize..50),
                              stop in proptest::option::of(-50isize..50),
                              step in prop_oneof![-7isize..=-1, 1isize..=7]) {
            let v: Vec<usize> = (0..len).collect();
            let s = SliceIndices::new(start, stop, Some(step), len).unwrap();
            prop_assert_eq!(take_slice(&v, &s), model_positions(len, start, stop, step));
        }

        #[test]
        fn delete_matches_model(len in 0usize..40, start in proptest::option::of(-50isize..50),
                                stop in proptest::option::of(-50isize..50),
                                step in prop_oneof![-7isize..=-1, 1isize..=7]) {
            let mut v: Vec<usize> = (0..len).collect();
            let s = SliceIndices::new(start, stop, Some(step), len).unwrap();
            let mut gone = model_positions(len, start, stop, step);
            gone.sort();
            let removed = delete_slice(&mut v, &s);
            let mut removed_sorted = removed.clone();
            removed_sorted.sort();
            prop_assert_eq!(removed_sorted, gone.clone());
            let expected: Vec<usize> = (0..len).filter(|i| !gone.contains(i)).collect();
            prop_assert_eq!(v, expected);
        }
    }
}
