//! Port of `re._compiler` (CPython 3.14 / 3.15): `_code(p, flags)` — the INFO block, the
//! pattern's code words and the final SUCCESS — exactly as CPython emits them.

use super::consts::*;
use super::parser::{Node, SetItem, SubId, Tree};
use super::{CaseMap, Decline, Env};

/// `iscased` / `tolower` / `fixes` of one `_compile` scope (`None` outside IGNORECASE).
#[derive(Clone, Copy)]
enum Fold<'e> {
    Ascii,
    Unicode(&'e dyn CaseMap),
}

impl Fold<'_> {
    /// `_sre.ascii_iscased` / `_sre.unicode_iscased`
    #[inline]
    fn iscased(self, c: u32) -> bool {
        match self {
            Fold::Ascii => c < 128 && (c as u8).is_ascii_alphabetic(),
            Fold::Unicode(m) => c != m.lower(c) || c != m.upper(c),
        }
    }
    /// `_sre.ascii_tolower` / `_sre.unicode_tolower`
    #[inline]
    fn lower(self, c: u32) -> u32 {
        match self {
            Fold::Ascii => {
                if c < 128 {
                    (c as u8).to_ascii_lowercase() as u32
                } else {
                    c
                }
            }
            Fold::Unicode(m) => m.lower(c),
        }
    }
}

/// `_get_iscased(flags)`
fn get_iscased<'e>(flags: u32, env: &'e Env<'e>) -> Option<Fold<'e>> {
    if flags & SRE_FLAG_IGNORECASE == 0 {
        None
    } else if flags & SRE_FLAG_UNICODE != 0 {
        Some(Fold::Unicode(env.case))
    } else {
        Some(Fold::Ascii)
    }
}

/// `_combine_flags(flags, add_flags, del_flags)`
#[inline]
fn combine_flags(mut flags: u32, add: u32, del: u32) -> u32 {
    if add & TYPE_FLAGS != 0 {
        flags &= !TYPE_FLAGS;
    }
    (flags | add) & !del
}

/// `MAXCODE` for `_sre.CODESIZE == 4`.
const MAXCODE: u128 = u32::MAX as u128;

struct Compiler<'t, 'e> {
    tree: &'t mut Tree,
    env: &'e Env<'e>,
    code: Vec<u32>,
}

impl<'e> Compiler<'_, 'e> {
    #[inline]
    fn emit(&mut self, w: u32) {
        self.code.push(w);
    }

    /// The `(fold, fixes)` of a `_compile(code, pattern, flags)` call.
    fn scope(&self, flags: u32) -> (Option<Fold<'e>>, bool) {
        if flags & SRE_FLAG_IGNORECASE != 0 && flags & SRE_FLAG_LOCALE == 0 {
            if flags & SRE_FLAG_UNICODE != 0 {
                (Some(Fold::Unicode(self.env.case)), !self.env.case.extra_is_empty())
            } else {
                (Some(Fold::Ascii), false)
            }
        } else {
            (None, false)
        }
    }

    /// `_compile(code, pattern, flags)`
    fn compile(&mut self, pattern: SubId, flags: u32) -> Result<(), Decline> {
        let (fold, fixes) = self.scope(flags);
        let n = self.tree.sub(pattern).data.len();
        for k in 0..n {
            let node = self.tree.sub(pattern).data[k].clone();
            match node {
                Node::Literal(av) | Node::NotLiteral(av) => {
                    let not = matches!(node, Node::NotLiteral(_));
                    let op = if not { NOT_LITERAL } else { LITERAL };
                    let Some(f) = fold else {
                        // not IGNORECASE (LOCALE never reaches the port)
                        self.emit(op);
                        self.emit(av);
                        continue;
                    };
                    if !f.iscased(av) {
                        self.emit(op);
                        self.emit(av);
                        continue;
                    }
                    let lo = f.lower(av);
                    if !fixes {
                        // ascii
                        self.emit(if not { NOT_LITERAL_IGNORE } else { LITERAL_IGNORE });
                        self.emit(lo);
                    } else if let Some(extra) = self.env.case.extra(lo) {
                        self.emit(IN_UNI_IGNORE);
                        let skip = self.code.len();
                        self.emit(0);
                        if not {
                            self.emit(NEGATE);
                        }
                        self.emit(LITERAL);
                        self.emit(lo);
                        for &k in extra {
                            self.emit(LITERAL);
                            self.emit(k);
                        }
                        self.emit(FAILURE);
                        self.code[skip] = (self.code.len() - skip) as u32;
                    } else {
                        self.emit(if not { NOT_LITERAL_UNI_IGNORE } else { LITERAL_UNI_IGNORE });
                        self.emit(lo);
                    }
                }
                Node::In(av) => {
                    let (charset, hascased) = optimize_charset(&av, fold, fixes, self.env);
                    if charset.is_empty() {
                        self.emit(FAILURE);
                    } else if charset.len() == 1 && charset[0] == SetItem::Negate {
                        // _CHARSET_ALL
                        self.emit(ANY_ALL);
                    } else {
                        if !hascased {
                            self.emit(IN);
                        } else if !fixes {
                            self.emit(IN_IGNORE);
                        } else {
                            self.emit(IN_UNI_IGNORE);
                        }
                        let skip = self.code.len();
                        self.emit(0);
                        compile_charset(&charset, flags, &mut self.code);
                        self.code[skip] = (self.code.len() - skip) as u32;
                    }
                }
                Node::Any => {
                    self.emit(if flags & SRE_FLAG_DOTALL != 0 { ANY_ALL } else { ANY });
                }
                Node::Repeat { op, min, max, item } => {
                    let (rep, until, one) = match op {
                        MIN_REPEAT => (REPEAT, MIN_UNTIL, MIN_REPEAT_ONE),
                        MAX_REPEAT => (REPEAT, MAX_UNTIL, REPEAT_ONE),
                        _ => (POSSESSIVE_REPEAT, SUCCESS, POSSESSIVE_REPEAT_ONE),
                    };
                    if self.simple(item) {
                        self.emit(one);
                        let skip = self.code.len();
                        self.emit(0);
                        self.emit(min);
                        self.emit(max);
                        self.compile(item, flags)?;
                        self.emit(SUCCESS);
                        self.code[skip] = (self.code.len() - skip) as u32;
                    } else {
                        self.emit(rep);
                        let skip = self.code.len();
                        self.emit(0);
                        self.emit(min);
                        self.emit(max);
                        self.compile(item, flags)?;
                        self.code[skip] = (self.code.len() - skip) as u32;
                        self.emit(until);
                    }
                }
                Node::Subpattern { group, add, del, p } => {
                    if let Some(g) = group.filter(|&g| g != 0) {
                        self.emit(MARK);
                        self.emit((g - 1) * 2);
                    }
                    self.compile(p, combine_flags(flags, add, del))?;
                    if let Some(g) = group.filter(|&g| g != 0) {
                        self.emit(MARK);
                        self.emit((g - 1) * 2 + 1);
                    }
                }
                Node::AtomicGroup(p) => {
                    self.emit(ATOMIC_GROUP);
                    let skip = self.code.len();
                    self.emit(0);
                    self.compile(p, flags)?;
                    self.emit(SUCCESS);
                    self.code[skip] = (self.code.len() - skip) as u32;
                }
                Node::Failure => self.emit(FAILURE),
                Node::Assert { dir, p } | Node::AssertNot { dir, p } => {
                    self.emit(if matches!(node, Node::Assert { .. }) { ASSERT } else { ASSERT_NOT });
                    let skip = self.code.len();
                    self.emit(0);
                    if dir >= 0 {
                        self.emit(0); // look ahead
                    } else {
                        let (lo, hi) = self.tree.getwidth(p);
                        if lo > MAXCODE {
                            return Err(Decline::Error); // looks too much behind
                        }
                        if lo != hi {
                            return Err(Decline::Error); // look-behind requires fixed-width pattern
                        }
                        self.emit(lo as u32); // look behind
                    }
                    self.compile(p, flags)?;
                    self.emit(SUCCESS);
                    self.code[skip] = (self.code.len() - skip) as u32;
                }
                Node::At(mut av) => {
                    self.emit(AT);
                    if flags & SRE_FLAG_MULTILINE != 0 {
                        av = at_multiline(av);
                    }
                    if flags & SRE_FLAG_UNICODE != 0 {
                        av = at_unicode(av);
                    }
                    self.emit(av);
                }
                Node::Branch(items) => {
                    self.emit(BRANCH);
                    let mut tail = Vec::with_capacity(items.len());
                    for av in items {
                        let skip = self.code.len();
                        self.emit(0);
                        self.compile(av, flags)?;
                        self.emit(JUMP);
                        tail.push(self.code.len());
                        self.emit(0);
                        self.code[skip] = (self.code.len() - skip) as u32;
                    }
                    self.emit(FAILURE); // end of branch
                    for t in tail {
                        self.code[t] = (self.code.len() - t) as u32;
                    }
                }
                Node::GroupRef(av) => {
                    if flags & SRE_FLAG_IGNORECASE == 0 {
                        self.emit(GROUPREF);
                    } else if !fixes {
                        self.emit(GROUPREF_IGNORE);
                    } else {
                        self.emit(GROUPREF_UNI_IGNORE);
                    }
                    self.emit(av - 1);
                }
                Node::GroupRefExists { cond, yes, no } => {
                    self.emit(GROUPREF_EXISTS);
                    self.emit(cond - 1);
                    let skipyes = self.code.len();
                    self.emit(0);
                    self.compile(yes, flags)?;
                    // `if av[2]:` (an empty SubPattern is falsy)
                    match no.filter(|&n| !self.tree.sub(n).data.is_empty()) {
                        Some(no) => {
                            self.emit(JUMP);
                            let skipno = self.code.len();
                            self.emit(0);
                            self.code[skipyes] = (self.code.len() - skipyes + 1) as u32;
                            self.compile(no, flags)?;
                            self.code[skipno] = (self.code.len() - skipno) as u32;
                        }
                        None => {
                            self.code[skipyes] = (self.code.len() - skipyes + 1) as u32;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `_simple(p)`
    fn simple(&self, p: SubId) -> bool {
        let data = &self.tree.sub(p).data;
        if data.len() != 1 {
            return false;
        }
        match &data[0] {
            Node::Subpattern { group, p, .. } => group.is_none() && self.simple(*p),
            Node::Literal(_) | Node::NotLiteral(_) | Node::Any | Node::In(_) => true,
            _ => false,
        }
    }

    /// `_get_literal_prefix(pattern, flags)`: (prefix, prefix_skip, got_all)
    fn get_literal_prefix(&self, pattern: SubId, flags: u32) -> (Vec<u32>, Option<usize>, bool) {
        let mut prefix = Vec::new();
        let mut prefix_skip = None;
        let iscased = get_iscased(flags, self.env);
        for node in &self.tree.sub(pattern).data {
            match node {
                Node::Literal(av) => {
                    if let Some(f) = iscased
                        && f.iscased(*av)
                    {
                        return (prefix, prefix_skip, false);
                    }
                    prefix.push(*av);
                }
                Node::Subpattern { group, add, del, p } => {
                    let flags1 = combine_flags(flags, *add, *del);
                    if flags1 & SRE_FLAG_IGNORECASE != 0 && flags1 & SRE_FLAG_LOCALE != 0 {
                        return (prefix, prefix_skip, false);
                    }
                    let (prefix1, prefix_skip1, got_all) = self.get_literal_prefix(*p, flags1);
                    if prefix_skip.is_none() {
                        if group.is_some() {
                            prefix_skip = Some(prefix.len());
                        } else if let Some(s1) = prefix_skip1 {
                            prefix_skip = Some(prefix.len() + s1);
                        }
                    }
                    prefix.extend(prefix1);
                    if !got_all {
                        return (prefix, prefix_skip, false);
                    }
                }
                _ => return (prefix, prefix_skip, false),
            }
        }
        (prefix, prefix_skip, true)
    }

    /// `_get_charset_prefix(pattern, flags)`
    fn get_charset_prefix(&self, mut pattern: SubId, mut flags: u32) -> Option<Vec<SetItem>> {
        let first = loop {
            let data = &self.tree.sub(pattern).data;
            let first = data.first()?;
            match first {
                Node::Subpattern { add, del, p, .. } => {
                    pattern = *p;
                    flags = combine_flags(flags, *add, *del);
                    if flags & SRE_FLAG_IGNORECASE != 0 && flags & SRE_FLAG_LOCALE != 0 {
                        return None;
                    }
                }
                other => break other,
            }
        };
        let iscased = get_iscased(flags, self.env);
        let cased = |c: u32| iscased.is_some_and(|f| f.iscased(c));
        match first {
            Node::Literal(av) => {
                if cased(*av) {
                    return None;
                }
                Some(vec![SetItem::Literal(*av)])
            }
            Node::Branch(items) => {
                let mut charset = Vec::with_capacity(items.len());
                for &p in items {
                    let first = self.tree.sub(p).data.first()?;
                    match first {
                        Node::Literal(av) if !cased(*av) => charset.push(SetItem::Literal(*av)),
                        _ => return None,
                    }
                }
                Some(charset)
            }
            Node::In(av) => {
                if let Some(f) = iscased {
                    for item in av {
                        match *item {
                            SetItem::Literal(c) => {
                                if f.iscased(c) {
                                    return None;
                                }
                            }
                            SetItem::Range(lo, hi) => {
                                if hi > 0xffff {
                                    return None;
                                }
                                if (lo..=hi).any(|c| f.iscased(c)) {
                                    return None;
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Some(av.clone())
            }
            _ => None,
        }
    }

    /// `_compile_info(code, pattern, flags)`
    fn compile_info(&mut self, pattern: SubId, flags: u32) {
        let (lo, mut hi) = self.tree.getwidth(pattern);
        if hi > MAXCODE {
            hi = MAXCODE;
        }
        if lo == 0 {
            self.code.extend([INFO, 4, 0, lo as u32, hi as u32]);
            return;
        }
        // look for a literal prefix (LOCALE never reaches the port)
        let (mut prefix, prefix_skip, got_all) = self.get_literal_prefix(pattern, flags);
        let mut charset = None;
        if prefix.is_empty()
            && let Some(cs) = self.get_charset_prefix(pattern, flags)
            && !cs.is_empty()
        {
            let (cs, _hascased) = optimize_charset(&cs, None, false, self.env);
            if !(cs.len() == 1 && cs[0] == SetItem::Negate) {
                charset = Some(cs);
            }
        }
        self.emit(INFO);
        let skip = self.code.len();
        self.emit(0);
        let mut mask = 0;
        if !prefix.is_empty() {
            mask = SRE_INFO_PREFIX;
            if prefix_skip.is_none() && got_all {
                mask |= SRE_INFO_LITERAL;
            }
        } else if charset.as_ref().is_some_and(|c| !c.is_empty()) {
            mask |= SRE_INFO_CHARSET;
        }
        self.emit(mask);
        if lo < MAXCODE {
            self.emit(lo as u32);
        } else {
            self.emit(MAXCODE as u32);
            prefix.truncate(MAXCODE as usize);
        }
        self.emit(hi as u32);
        if !prefix.is_empty() {
            self.emit(prefix.len() as u32);
            let prefix_skip = prefix_skip.unwrap_or(prefix.len());
            self.emit(prefix_skip as u32);
            self.code.extend_from_slice(&prefix);
            self.code.extend(generate_overlap_table(&prefix));
        } else if let Some(cs) = charset.filter(|c| !c.is_empty()) {
            compile_charset(&cs, flags, &mut self.code);
        }
        self.code[skip] = (self.code.len() - skip) as u32;
    }
}

/// `_generate_overlap_table(prefix)`
fn generate_overlap_table(prefix: &[u32]) -> Vec<u32> {
    let mut table = vec![0u32; prefix.len()];
    for i in 1..prefix.len() {
        let mut idx = table[i - 1] as usize;
        loop {
            if prefix[i] == prefix[idx] {
                table[i] = idx as u32 + 1;
                break;
            }
            if idx == 0 {
                table[i] = 0;
                break;
            }
            idx = table[idx - 1] as usize;
        }
    }
    table
}

/// `_compile_charset(charset, flags, code)`
fn compile_charset(charset: &[SetItem], flags: u32, code: &mut Vec<u32>) {
    for item in charset {
        match item {
            SetItem::Negate => code.push(NEGATE),
            SetItem::Literal(v) => code.extend([LITERAL, *v]),
            SetItem::Range(lo, hi) => code.extend([RANGE, *lo, *hi]),
            SetItem::RangeUniIgnore(lo, hi) => code.extend([RANGE_UNI_IGNORE, *lo, *hi]),
            SetItem::Charset(words) => {
                code.push(CHARSET);
                code.extend_from_slice(words);
            }
            SetItem::BigCharset(words) => {
                code.push(BIGCHARSET);
                code.extend_from_slice(words);
            }
            SetItem::Category(c) => {
                code.push(CATEGORY);
                // LOCALE never reaches the port
                code.push(if flags & SRE_FLAG_UNICODE != 0 { ch_unicode(*c) } else { *c });
            }
        }
    }
    code.push(FAILURE);
}

/// `bytearray` charmap with CPython's `IndexError` on out-of-range stores.
struct Charmap(Vec<u8>);

impl Charmap {
    #[inline]
    fn set(&mut self, i: u32) -> Result<(), ()> {
        match self.0.get_mut(i as usize) {
            Some(b) => {
                *b = 1;
                Ok(())
            }
            None => Err(()),
        }
    }
    fn find(&self, value: u8, from: usize) -> Option<usize> {
        self.0.get(from..)?.iter().position(|&b| b == value).map(|p| p + from)
    }
}

/// `_mk_bitmap(bits)`: 32-bit words, bit `k` of word `w` = `bits[32 * w + k]`.
fn mk_bitmap(bits: &[u8]) -> Vec<u32> {
    bits.chunks(32)
        .map(|chunk| chunk.iter().enumerate().fold(0u32, |w, (k, &b)| if b != 0 { w | (1 << k) } else { w }))
        .collect()
}

/// The stores of one LITERAL or RANGE item of `_optimize_charset` (the body of its `try`):
/// `Err` is CPython's `IndexError` (a code beyond the charmap). `cur` is the loop's
/// `(op, av)`: LITERAL rebinds `av = fixup(av)` before storing, and a retry keeps it.
fn store(
    cur: &mut SetItem,
    charmap: &mut Charmap,
    hascased: &mut bool,
    fold: Option<Fold<'_>>,
    fixes: bool,
    env: &Env<'_>,
) -> Result<(), ()> {
    let extra = |c: u32| if fixes { env.case.extra(c) } else { None };
    match cur {
        SetItem::Literal(av) => {
            if let Some(f) = fold {
                *av = f.lower(*av);
                charmap.set(*av)?;
                if let Some(ks) = extra(*av) {
                    for &k in ks {
                        charmap.set(k)?;
                    }
                }
                if !*hascased && f.iscased(*av) {
                    *hascased = true;
                }
            } else {
                charmap.set(*av)?;
            }
        }
        SetItem::Range(lo, hi) => {
            let (lo, hi) = (*lo, *hi);
            if let Some(f) = fold {
                if fixes {
                    for c in lo..=hi {
                        let i = f.lower(c);
                        charmap.set(i)?;
                        if let Some(ks) = extra(i) {
                            for &k in ks {
                                charmap.set(k)?;
                            }
                        }
                    }
                } else {
                    for c in lo..=hi {
                        charmap.set(f.lower(c))?;
                    }
                }
                if !*hascased {
                    *hascased = (lo..=hi).any(|c| f.iscased(c));
                }
            } else {
                for c in lo..=hi {
                    charmap.set(c)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// `_optimize_charset(charset, iscased, fixup, fixes)`: (charset, hascased).
fn optimize_charset(charset: &[SetItem], fold: Option<Fold<'_>>, fixes: bool, env: &Env<'_>) -> (Vec<SetItem>, bool) {
    let mut out: Vec<SetItem> = Vec::new();
    let mut tail: Vec<SetItem> = Vec::new();
    let mut charmap = Charmap(vec![0u8; 256]);
    let mut hascased = false;
    for item in charset {
        match item {
            SetItem::Negate => {
                out.push(SetItem::Negate);
                continue;
            }
            SetItem::Category(c) => {
                if !tail.is_empty() && tail.contains(&SetItem::Category(ch_negate(*c))) {
                    // Optimize [\s\S] etc.
                    return (if out.is_empty() { vec![SetItem::Negate] } else { Vec::new() }, false);
                }
                tail.push(item.clone());
                continue;
            }
            SetItem::Literal(_) | SetItem::Range(..) => {}
            other => {
                tail.push(other.clone());
                continue;
            }
        }
        let mut cur = item.clone();
        loop {
            if store(&mut cur, &mut charmap, &mut hascased, fold, fixes, env).is_ok() {
                break;
            }
            if charmap.0.len() == 256 {
                // character set contains non-UCS1 character codes
                charmap.0.resize(0x10000, 0);
                continue;
            }
            // character set contains non-BMP character codes
            if let Some(f) = fold {
                match cur {
                    SetItem::Range(lo, hi) => {
                        if fixes {
                            cur = SetItem::RangeUniIgnore(lo, hi);
                        }
                        hascased = true;
                    }
                    SetItem::Literal(av) => {
                        if !hascased && f.iscased(av) {
                            hascased = true;
                        }
                    }
                    _ => unreachable!("only LITERAL and RANGE store into the charmap"),
                }
            }
            tail.push(cur);
            break;
        }
    }

    // compress character map
    let mut runs: Option<Vec<(usize, usize)>> = Some(Vec::new());
    let mut q = 0;
    while let Some(p) = charmap.find(1, q) {
        let r = runs.as_mut().expect("set above");
        if r.len() >= 2 {
            runs = None;
            break;
        }
        match charmap.find(0, p) {
            None => {
                r.push((p, charmap.0.len()));
                break;
            }
            Some(z) => {
                r.push((p, z));
                q = z;
            }
        }
    }
    if let Some(runs) = runs {
        // use literal/range
        for (p, q) in runs {
            if q - p == 1 {
                out.push(SetItem::Literal(p as u32));
            } else {
                out.push(SetItem::Range(p as u32, (q - 1) as u32));
            }
        }
        out.extend(tail);
        // if the case was changed or new representation is more compact
        if hascased || out.len() < charset.len() {
            return (out, hascased);
        }
        // else original character set is good enough
        return (charset.to_vec(), hascased);
    }

    // use bitmap
    if charmap.0.len() == 256 {
        out.push(SetItem::Charset(mk_bitmap(&charmap.0)));
        out.extend(tail);
        return (out, hascased);
    }

    // BIGCHARSET: unique 256-char chunks, a 256-byte chunk index, the chunks' bitmaps
    let mut comps: std::collections::HashMap<&[u8], u8> = std::collections::HashMap::new();
    let mut mapping = [0u8; 256];
    let mut data: Vec<u8> = Vec::new();
    let mut block: u32 = 0;
    for (i, chunk) in charmap.0.chunks(256).enumerate() {
        if let Some(&b) = comps.get(chunk) {
            mapping[i] = b;
        } else {
            // at most 256 chunks: block numbers stay below 256
            mapping[i] = block as u8;
            comps.insert(chunk, block as u8);
            block += 1;
            data.extend_from_slice(chunk);
        }
    }
    let mut words = Vec::with_capacity(1 + 64 + data.len() / 32);
    words.push(block);
    // `_bytes_to_codes(mapping)`: native-endian 32-bit words
    words.extend(mapping.chunks(4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]])));
    words.extend(mk_bitmap(&data));
    out.push(SetItem::BigCharset(words));
    out.extend(tail);
    (out, hascased)
}

/// `_code(p, flags)`: the code words.
pub(crate) fn code(tree: &mut Tree, flags: u32, env: &Env<'_>) -> Result<Vec<u32>, Decline> {
    let flags = tree.state.flags | flags;
    let root = tree.root;
    let mut c = Compiler { tree, env, code: Vec::with_capacity(64) };
    c.compile_info(root, flags);
    c.compile(root, flags)?;
    c.emit(SUCCESS);
    Ok(c.code)
}
