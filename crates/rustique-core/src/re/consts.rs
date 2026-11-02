//! `re._constants` (CPython 3.14 and 3.15 share it: `MAGIC = 20230612`): opcodes, position
//! and category codes, flags. The glue checks every value against the running interpreter's
//! `re._constants` at install.

#![allow(dead_code)]

/// `_constants.MAGIC` this port was written against.
pub const MAGIC: u32 = 20230612;

// OPCODES, in `_makecodes` order.
pub const FAILURE: u32 = 0;
pub const SUCCESS: u32 = 1;
pub const ANY: u32 = 2;
pub const ANY_ALL: u32 = 3;
pub const ASSERT: u32 = 4;
pub const ASSERT_NOT: u32 = 5;
pub const AT: u32 = 6;
pub const BRANCH: u32 = 7;
pub const CATEGORY: u32 = 8;
pub const CHARSET: u32 = 9;
pub const BIGCHARSET: u32 = 10;
pub const GROUPREF: u32 = 11;
pub const GROUPREF_EXISTS: u32 = 12;
pub const IN: u32 = 13;
pub const INFO: u32 = 14;
pub const JUMP: u32 = 15;
pub const LITERAL: u32 = 16;
pub const MARK: u32 = 17;
pub const MAX_UNTIL: u32 = 18;
pub const MIN_UNTIL: u32 = 19;
pub const NOT_LITERAL: u32 = 20;
pub const NEGATE: u32 = 21;
pub const RANGE: u32 = 22;
pub const REPEAT: u32 = 23;
pub const REPEAT_ONE: u32 = 24;
pub const SUBPATTERN: u32 = 25;
pub const MIN_REPEAT_ONE: u32 = 26;
pub const ATOMIC_GROUP: u32 = 27;
pub const POSSESSIVE_REPEAT: u32 = 28;
pub const POSSESSIVE_REPEAT_ONE: u32 = 29;
pub const GROUPREF_IGNORE: u32 = 30;
pub const IN_IGNORE: u32 = 31;
pub const LITERAL_IGNORE: u32 = 32;
pub const NOT_LITERAL_IGNORE: u32 = 33;
pub const GROUPREF_LOC_IGNORE: u32 = 34;
pub const IN_LOC_IGNORE: u32 = 35;
pub const LITERAL_LOC_IGNORE: u32 = 36;
pub const NOT_LITERAL_LOC_IGNORE: u32 = 37;
pub const GROUPREF_UNI_IGNORE: u32 = 38;
pub const IN_UNI_IGNORE: u32 = 39;
pub const LITERAL_UNI_IGNORE: u32 = 40;
pub const NOT_LITERAL_UNI_IGNORE: u32 = 41;
pub const RANGE_UNI_IGNORE: u32 = 42;
/// Parser-only (deleted from `OPCODES`).
pub const MIN_REPEAT: u32 = 43;
pub const MAX_REPEAT: u32 = 44;

// ATCODES
pub const AT_BEGINNING: u32 = 0;
pub const AT_BEGINNING_LINE: u32 = 1;
pub const AT_BEGINNING_STRING: u32 = 2;
pub const AT_BOUNDARY: u32 = 3;
pub const AT_NON_BOUNDARY: u32 = 4;
pub const AT_END: u32 = 5;
pub const AT_END_LINE: u32 = 6;
pub const AT_END_STRING: u32 = 7;
pub const AT_LOC_BOUNDARY: u32 = 8;
pub const AT_LOC_NON_BOUNDARY: u32 = 9;
pub const AT_UNI_BOUNDARY: u32 = 10;
pub const AT_UNI_NON_BOUNDARY: u32 = 11;

// CHCODES
pub const CATEGORY_DIGIT: u32 = 0;
pub const CATEGORY_NOT_DIGIT: u32 = 1;
pub const CATEGORY_SPACE: u32 = 2;
pub const CATEGORY_NOT_SPACE: u32 = 3;
pub const CATEGORY_WORD: u32 = 4;
pub const CATEGORY_NOT_WORD: u32 = 5;
pub const CATEGORY_LINEBREAK: u32 = 6;
pub const CATEGORY_NOT_LINEBREAK: u32 = 7;
pub const CATEGORY_LOC_WORD: u32 = 8;
pub const CATEGORY_LOC_NOT_WORD: u32 = 9;
pub const CATEGORY_UNI_DIGIT: u32 = 10;
pub const CATEGORY_UNI_NOT_DIGIT: u32 = 11;
pub const CATEGORY_UNI_SPACE: u32 = 12;
pub const CATEGORY_UNI_NOT_SPACE: u32 = 13;
pub const CATEGORY_UNI_WORD: u32 = 14;
pub const CATEGORY_UNI_NOT_WORD: u32 = 15;
pub const CATEGORY_UNI_LINEBREAK: u32 = 16;
pub const CATEGORY_UNI_NOT_LINEBREAK: u32 = 17;

// flags
pub const SRE_FLAG_IGNORECASE: u32 = 2;
pub const SRE_FLAG_LOCALE: u32 = 4;
pub const SRE_FLAG_MULTILINE: u32 = 8;
pub const SRE_FLAG_DOTALL: u32 = 16;
pub const SRE_FLAG_UNICODE: u32 = 32;
pub const SRE_FLAG_VERBOSE: u32 = 64;
pub const SRE_FLAG_DEBUG: u32 = 128;
pub const SRE_FLAG_ASCII: u32 = 256;

/// `_parser.TYPE_FLAGS`
pub const TYPE_FLAGS: u32 = SRE_FLAG_ASCII | SRE_FLAG_LOCALE | SRE_FLAG_UNICODE;
/// `_parser.GLOBAL_FLAGS`
pub const GLOBAL_FLAGS: u32 = SRE_FLAG_DEBUG;

// flags of the INFO block
pub const SRE_INFO_PREFIX: u32 = 1;
pub const SRE_INFO_LITERAL: u32 = 2;
pub const SRE_INFO_CHARSET: u32 = 4;

/// Every opcode name with its value (`_constants.OPCODES` plus the two parser-only codes),
/// for the install-time check against the running interpreter.
pub const OPCODE_NAMES: &[(&str, u32)] = &[
    ("FAILURE", FAILURE),
    ("SUCCESS", SUCCESS),
    ("ANY", ANY),
    ("ANY_ALL", ANY_ALL),
    ("ASSERT", ASSERT),
    ("ASSERT_NOT", ASSERT_NOT),
    ("AT", AT),
    ("BRANCH", BRANCH),
    ("CATEGORY", CATEGORY),
    ("CHARSET", CHARSET),
    ("BIGCHARSET", BIGCHARSET),
    ("GROUPREF", GROUPREF),
    ("GROUPREF_EXISTS", GROUPREF_EXISTS),
    ("IN", IN),
    ("INFO", INFO),
    ("JUMP", JUMP),
    ("LITERAL", LITERAL),
    ("MARK", MARK),
    ("MAX_UNTIL", MAX_UNTIL),
    ("MIN_UNTIL", MIN_UNTIL),
    ("NOT_LITERAL", NOT_LITERAL),
    ("NEGATE", NEGATE),
    ("RANGE", RANGE),
    ("REPEAT", REPEAT),
    ("REPEAT_ONE", REPEAT_ONE),
    ("SUBPATTERN", SUBPATTERN),
    ("MIN_REPEAT_ONE", MIN_REPEAT_ONE),
    ("ATOMIC_GROUP", ATOMIC_GROUP),
    ("POSSESSIVE_REPEAT", POSSESSIVE_REPEAT),
    ("POSSESSIVE_REPEAT_ONE", POSSESSIVE_REPEAT_ONE),
    ("GROUPREF_IGNORE", GROUPREF_IGNORE),
    ("IN_IGNORE", IN_IGNORE),
    ("LITERAL_IGNORE", LITERAL_IGNORE),
    ("NOT_LITERAL_IGNORE", NOT_LITERAL_IGNORE),
    ("GROUPREF_LOC_IGNORE", GROUPREF_LOC_IGNORE),
    ("IN_LOC_IGNORE", IN_LOC_IGNORE),
    ("LITERAL_LOC_IGNORE", LITERAL_LOC_IGNORE),
    ("NOT_LITERAL_LOC_IGNORE", NOT_LITERAL_LOC_IGNORE),
    ("GROUPREF_UNI_IGNORE", GROUPREF_UNI_IGNORE),
    ("IN_UNI_IGNORE", IN_UNI_IGNORE),
    ("LITERAL_UNI_IGNORE", LITERAL_UNI_IGNORE),
    ("NOT_LITERAL_UNI_IGNORE", NOT_LITERAL_UNI_IGNORE),
    ("RANGE_UNI_IGNORE", RANGE_UNI_IGNORE),
    ("MIN_REPEAT", MIN_REPEAT),
    ("MAX_REPEAT", MAX_REPEAT),
];

/// `_constants.ATCODES` names and values.
pub const ATCODE_NAMES: &[(&str, u32)] = &[
    ("AT_BEGINNING", AT_BEGINNING),
    ("AT_BEGINNING_LINE", AT_BEGINNING_LINE),
    ("AT_BEGINNING_STRING", AT_BEGINNING_STRING),
    ("AT_BOUNDARY", AT_BOUNDARY),
    ("AT_NON_BOUNDARY", AT_NON_BOUNDARY),
    ("AT_END", AT_END),
    ("AT_END_LINE", AT_END_LINE),
    ("AT_END_STRING", AT_END_STRING),
    ("AT_LOC_BOUNDARY", AT_LOC_BOUNDARY),
    ("AT_LOC_NON_BOUNDARY", AT_LOC_NON_BOUNDARY),
    ("AT_UNI_BOUNDARY", AT_UNI_BOUNDARY),
    ("AT_UNI_NON_BOUNDARY", AT_UNI_NON_BOUNDARY),
];

/// `_constants.CHCODES` names and values.
pub const CHCODE_NAMES: &[(&str, u32)] = &[
    ("CATEGORY_DIGIT", CATEGORY_DIGIT),
    ("CATEGORY_NOT_DIGIT", CATEGORY_NOT_DIGIT),
    ("CATEGORY_SPACE", CATEGORY_SPACE),
    ("CATEGORY_NOT_SPACE", CATEGORY_NOT_SPACE),
    ("CATEGORY_WORD", CATEGORY_WORD),
    ("CATEGORY_NOT_WORD", CATEGORY_NOT_WORD),
    ("CATEGORY_LINEBREAK", CATEGORY_LINEBREAK),
    ("CATEGORY_NOT_LINEBREAK", CATEGORY_NOT_LINEBREAK),
    ("CATEGORY_LOC_WORD", CATEGORY_LOC_WORD),
    ("CATEGORY_LOC_NOT_WORD", CATEGORY_LOC_NOT_WORD),
    ("CATEGORY_UNI_DIGIT", CATEGORY_UNI_DIGIT),
    ("CATEGORY_UNI_NOT_DIGIT", CATEGORY_UNI_NOT_DIGIT),
    ("CATEGORY_UNI_SPACE", CATEGORY_UNI_SPACE),
    ("CATEGORY_UNI_NOT_SPACE", CATEGORY_UNI_NOT_SPACE),
    ("CATEGORY_UNI_WORD", CATEGORY_UNI_WORD),
    ("CATEGORY_UNI_NOT_WORD", CATEGORY_UNI_NOT_WORD),
    ("CATEGORY_UNI_LINEBREAK", CATEGORY_UNI_LINEBREAK),
    ("CATEGORY_UNI_NOT_LINEBREAK", CATEGORY_UNI_NOT_LINEBREAK),
];

/// Flag names and values (`SRE_FLAG_*`, `SRE_INFO_*`).
pub const FLAG_NAMES: &[(&str, u32)] = &[
    ("SRE_FLAG_IGNORECASE", SRE_FLAG_IGNORECASE),
    ("SRE_FLAG_LOCALE", SRE_FLAG_LOCALE),
    ("SRE_FLAG_MULTILINE", SRE_FLAG_MULTILINE),
    ("SRE_FLAG_DOTALL", SRE_FLAG_DOTALL),
    ("SRE_FLAG_UNICODE", SRE_FLAG_UNICODE),
    ("SRE_FLAG_VERBOSE", SRE_FLAG_VERBOSE),
    ("SRE_FLAG_DEBUG", SRE_FLAG_DEBUG),
    ("SRE_FLAG_ASCII", SRE_FLAG_ASCII),
    ("SRE_INFO_PREFIX", SRE_INFO_PREFIX),
    ("SRE_INFO_LITERAL", SRE_INFO_LITERAL),
    ("SRE_INFO_CHARSET", SRE_INFO_CHARSET),
];

/// `CH_NEGATE`: even codes pair with the following odd code.
#[inline]
pub fn ch_negate(c: u32) -> u32 {
    c ^ 1
}

/// `CH_UNICODE[c]` (defined for the eight base categories).
#[inline]
pub fn ch_unicode(c: u32) -> u32 {
    debug_assert!(c <= CATEGORY_NOT_LINEBREAK);
    c + CATEGORY_UNI_DIGIT
}

/// `AT_MULTILINE.get(av, av)`
#[inline]
pub fn at_multiline(a: u32) -> u32 {
    match a {
        AT_BEGINNING => AT_BEGINNING_LINE,
        AT_END => AT_END_LINE,
        x => x,
    }
}

/// `AT_UNICODE.get(av, av)`
#[inline]
pub fn at_unicode(a: u32) -> u32 {
    match a {
        AT_BOUNDARY => AT_UNI_BOUNDARY,
        AT_NON_BOUNDARY => AT_UNI_NON_BOUNDARY,
        x => x,
    }
}
