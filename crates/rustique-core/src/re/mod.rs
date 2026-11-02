//! A port of CPython's regular-expression compiler (`re._parser` + `re._compiler`, 3.14 and
//! 3.15): for a pattern and flags, exactly the arguments `re._compiler.compile` passes to
//! `_sre.compile` — the final flags, the code words, the group count and the group names —
//! or a [`Decline`] when CPython's own compiler must run (the pattern raises, warns, needs
//! `unicodedata`, uses `LOCALE`, `DEBUG` or nests deeper than the port recurses).
//!
//! Everything that depends on the running interpreter comes in through [`Env`]: `_sre`'s
//! `MAXREPEAT`/`MAXGROUPS` and the Unicode case mappings (`_sre.unicode_tolower`, the
//! uppercase mapping behind `_sre.unicode_iscased`, `re._casefix._EXTRA_CASES`), so the
//! result follows the running CPython's Unicode database.
//!
//! [`template`] validates `re.sub` replacement templates the same way
//! (`re._parser.parse_template`).

pub mod consts;
mod compiler;
mod parser;
pub mod template;

#[cfg(test)]
mod tests;

use consts::*;

/// Why the port leaves a pattern to CPython.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decline {
    /// CPython raises (`re.error`, `OverflowError`, `ValueError`, ...).
    Error,
    /// CPython warns (`FutureWarning` about possible set operations or nested sets).
    Warning,
    /// Valid, but outside the port: `\N{...}`, `LOCALE`, `DEBUG`, unknown flag bits,
    /// non-ASCII group names, deep nesting.
    Unsupported,
}

/// The running interpreter's case mappings (simple, one code point to one).
pub trait CaseMap {
    /// `Py_UNICODE_TOLOWER(c)` (`_sre.unicode_tolower`).
    fn lower(&self, c: u32) -> u32;
    /// `Py_UNICODE_TOUPPER(c)` (with `lower`, `_sre.unicode_iscased`).
    fn upper(&self, c: u32) -> u32;
    /// `re._casefix._EXTRA_CASES.get(c)`.
    fn extra(&self, c: u32) -> Option<&[u32]>;
    /// `not _EXTRA_CASES`
    fn extra_is_empty(&self) -> bool;
}

/// What the compiler reads from the running interpreter.
pub struct Env<'a> {
    /// `_sre.MAXREPEAT`
    pub maxrepeat: u32,
    /// `_sre.MAXGROUPS`
    pub maxgroups: u32,
    pub case: &'a dyn CaseMap,
}

/// `_casefix._EXTRA_CASES` as a sorted table.
#[derive(Debug, Clone, Default)]
pub struct ExtraCases {
    keys: Vec<u32>,
    values: Vec<Vec<u32>>,
}

impl ExtraCases {
    /// From `(key, values)` pairs in any order.
    pub fn new(mut pairs: Vec<(u32, Vec<u32>)>) -> Self {
        pairs.sort_by_key(|p| p.0);
        pairs.dedup_by_key(|p| p.0);
        let (keys, values) = pairs.into_iter().unzip();
        ExtraCases { keys, values }
    }
    #[inline]
    pub fn get(&self, c: u32) -> Option<&[u32]> {
        self.keys.binary_search(&c).ok().map(|i| self.values[i].as_slice())
    }
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

/// The arguments of `_sre.compile(pattern, flags, code, groups, groupindex, indexgroup)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compiled {
    /// `flags | p.state.flags`
    pub flags: u32,
    pub code: Vec<u32>,
    /// `p.state.groups - 1`
    pub groups: u32,
    /// `p.state.groupdict` in insertion order: (name, group number). `indexgroup` is the
    /// tuple of `groups + 1` entries holding each name at its number, `None` elsewhere.
    pub groupindex: Vec<(String, u32)>,
}

/// Flags the port compiles: IGNORECASE, MULTILINE, DOTALL, UNICODE, VERBOSE, ASCII.
/// LOCALE and DEBUG (and unknown bits) go to CPython.
pub const SUPPORTED_FLAGS: u32 =
    SRE_FLAG_IGNORECASE | SRE_FLAG_MULTILINE | SRE_FLAG_DOTALL | SRE_FLAG_UNICODE | SRE_FLAG_VERBOSE | SRE_FLAG_ASCII;

/// `re._compiler.compile(p, flags)` for `p` a `str` (`istext`) or `bytes` (its bytes as code
/// units): the `_sre.compile` arguments, or why CPython must compile it.
pub fn compile(pattern: &[u32], istext: bool, flags: u32, env: &Env<'_>) -> Result<Compiled, Decline> {
    if flags & !SUPPORTED_FLAGS != 0 {
        return Err(Decline::Unsupported);
    }
    let mut tree = parser::parse(pattern, istext, flags, env)?;
    if tree.state.flags & SRE_FLAG_LOCALE != 0 {
        return Err(Decline::Unsupported);
    }
    let code = compiler::code(&mut tree, flags, env)?;
    let state = tree.state;
    Ok(Compiled { flags: flags | state.flags, code, groups: state.groups() - 1, groupindex: state.groupdict })
}
