//! Date/time text, Python-independent: CPython's `_strptime` parsing and `datetime`/`time`
//! `strftime` formatting for exact subsets (see each module), plus calendar arithmetic.
//! The `rustique-py` datetime targets decide when an input is in the subset at the Python
//! level (types, locale, tzinfo) and fall back to CPython otherwise.

pub mod cal;
pub mod strftime;
pub mod strptime;
