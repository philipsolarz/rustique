//! The verified CPython builds: the only binaries the interp tier swaps handlers in.
//!
//! One entry per (release, configuration, ELF build-id), written by
//! `tools/record_build.py --write` after CPython's full regression suite passed with the tier
//! on for that build (docs/targets/interp.md). Addresses are link-time virtual addresses in
//! the object that holds the interpreter (the executable, or libpython); at run time they are
//! relocated by that object's load bias. A stripped variant of a build has the same build-id
//! and the same addresses.

/// One verified build.
#[derive(Debug)]
pub struct VerifiedBuild {
    /// The CPython release (`"3.15.1"`); the handler set generated from it is used.
    pub release: &'static str,
    /// `Py_Version` of the release (final).
    pub hexversion: u32,
    pub free_threaded: bool,
    /// NT_GNU_BUILD_ID of the object holding the interpreter, lowercase hex.
    pub build_id: &'static str,
    /// Where the build comes from.
    pub source: &'static str,
    /// `instruction_funcptr_handler_table` (the core's tail-call dispatch table).
    pub handler_table: usize,
    /// `instruction_funcptr_tracing_table` (0 when the build has no tier 2).
    pub tracing_table: usize,
    /// (opcode, `_TAIL_CALL_<op>` of the core) for every handler the tier may swap, and NOP.
    pub handlers: &'static [(u8, usize)],
}

pub static VERIFIED: &[VerifiedBuild] = &[
    // BEGIN VERIFIED BUILDS
    VerifiedBuild {
        release: "3.15.1",
        hexversion: 0x030f01f0,
        free_threaded: false,
        build_id: "f8319cf30969afb31dfb74909ae66e85b6ca2c9f",
        source: "python-build-standalone cpython-3.15.1-linux-x86_64-gnu (uv)",
        handler_table: 0x14cdbf0,
        tracing_table: 0x14ce400,
        handlers: &[
            (79, 0x1818612),  // LOAD_ATTR
            (186, 0x1801396), // LOAD_ATTR_INSTANCE_VALUE
            (190, 0x18046d0), // LOAD_ATTR_MODULE
            (195, 0x181851c), // LOAD_ATTR_WITH_HINT
            (194, 0x1800d00), // LOAD_ATTR_SLOT
            (183, 0x180a8c6), // LOAD_ATTR_CLASS
            (184, 0x181830e), // LOAD_ATTR_CLASS_WITH_METACLASS_CHECK
            (193, 0x183f9c6), // LOAD_ATTR_PROPERTY
            (185, 0x1316240), // LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN
            (189, 0x1801040), // LOAD_ATTR_METHOD_WITH_VALUES
            (188, 0x1801140), // LOAD_ATTR_METHOD_NO_DICT
            (187, 0x1802e1a), // LOAD_ATTR_METHOD_LAZY_DICT
            (192, 0x1804800), // LOAD_ATTR_NONDESCRIPTOR_WITH_VALUES
            (191, 0x191b19e), // LOAD_ATTR_NONDESCRIPTOR_NO_DICT
            (163, 0x1808300), // CALL_NON_PY_GENERAL
            (155, 0x181cc8e), // CALL_KW_NON_PY
            (153, 0x1806b5a), // CALL_ISINSTANCE
            (25, 0x1801800),  // NOP
        ],
    },
    VerifiedBuild {
        release: "3.15.1",
        hexversion: 0x030f01f0,
        free_threaded: true,
        build_id: "73cfc3d07b7363a52eb01583789c7c1421403151",
        source: "python-build-standalone cpython-3.15.1+freethreaded-linux-x86_64-gnu (uv)",
        handler_table: 0x1493a90,
        tracing_table: 0x0,
        handlers: &[
            (79, 0x1832788),  // LOAD_ATTR
            (186, 0x1801a0c), // LOAD_ATTR_INSTANCE_VALUE
            (190, 0x1807bc0), // LOAD_ATTR_MODULE
            (195, 0x1832580), // LOAD_ATTR_WITH_HINT
            (194, 0x18041da), // LOAD_ATTR_SLOT
            (183, 0x180ec40), // LOAD_ATTR_CLASS
            (184, 0x1882614), // LOAD_ATTR_CLASS_WITH_METACLASS_CHECK
            (193, 0x1840e40), // LOAD_ATTR_PROPERTY
            (185, 0x1308270), // LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN
            (189, 0x1801e84), // LOAD_ATTR_METHOD_WITH_VALUES
            (188, 0x1801b80), // LOAD_ATTR_METHOD_NO_DICT
            (187, 0x180c94c), // LOAD_ATTR_METHOD_LAZY_DICT
            (192, 0x18326c0), // LOAD_ATTR_NONDESCRIPTOR_WITH_VALUES
            (191, 0x193bd1e), // LOAD_ATTR_NONDESCRIPTOR_NO_DICT
            (163, 0x180bb04), // CALL_NON_PY_GENERAL
            (155, 0x181ed00), // CALL_KW_NON_PY
            (153, 0x1809e80), // CALL_ISINSTANCE
            (25, 0x18018d2),  // NOP
        ],
    },
    VerifiedBuild {
        release: "3.15.2",
        hexversion: 0x030f02f0,
        free_threaded: false,
        build_id: "d6d079626c440c7967e34959881b88b96d0fdabb",
        source: "python-build-standalone cpython-3.15.2-linux-x86_64-gnu (uv)",
        handler_table: 0x14d21d0,
        tracing_table: 0x14d29e0,
        handlers: &[
            (79, 0x180e64a),  // LOAD_ATTR
            (186, 0x1801310), // LOAD_ATTR_INSTANCE_VALUE
            (190, 0x1804b90), // LOAD_ATTR_MODULE
            (195, 0x180e554), // LOAD_ATTR_WITH_HINT
            (194, 0x180168c), // LOAD_ATTR_SLOT
            (183, 0x180e240), // LOAD_ATTR_CLASS
            (184, 0x180e340), // LOAD_ATTR_CLASS_WITH_METACLASS_CHECK
            (193, 0x182b880), // LOAD_ATTR_PROPERTY
            (185, 0x1318040), // LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN
            (189, 0x1800f86), // LOAD_ATTR_METHOD_WITH_VALUES
            (188, 0x1801080), // LOAD_ATTR_METHOD_NO_DICT
            (187, 0x1809340), // LOAD_ATTR_METHOD_LAZY_DICT
            (192, 0x1804cc0), // LOAD_ATTR_NONDESCRIPTOR_WITH_VALUES
            (191, 0x191185e), // LOAD_ATTR_NONDESCRIPTOR_NO_DICT
            (163, 0x1807046), // CALL_NON_PY_GENERAL
            (155, 0x181e9c4), // CALL_KW_NON_PY
            (153, 0x18078da), // CALL_ISINSTANCE
            (25, 0x1800d00),  // NOP
        ],
    },
    VerifiedBuild {
        release: "3.15.2",
        hexversion: 0x030f02f0,
        free_threaded: true,
        build_id: "56f51dd9c5d24b26889bb5465fa6b9713350079e",
        source: "python-build-standalone cpython-3.15.2+freethreaded-linux-x86_64-gnu (uv)",
        handler_table: 0x149b500,
        tracing_table: 0x0,
        handlers: &[
            (79, 0x18339cc),  // LOAD_ATTR
            (186, 0x1802080), // LOAD_ATTR_INSTANCE_VALUE
            (190, 0x1807900), // LOAD_ATTR_MODULE
            (195, 0x18337de), // LOAD_ATTR_WITH_HINT
            (194, 0x1804d1a), // LOAD_ATTR_SLOT
            (183, 0x18102d8), // LOAD_ATTR_CLASS
            (184, 0x186bdd4), // LOAD_ATTR_CLASS_WITH_METACLASS_CHECK
            (193, 0x183d580), // LOAD_ATTR_PROPERTY
            (185, 0x130dc10), // LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN
            (189, 0x1801c8c), // LOAD_ATTR_METHOD_WITH_VALUES
            (188, 0x1801d90), // LOAD_ATTR_METHOD_NO_DICT
            (187, 0x18034d0), // LOAD_ATTR_METHOD_LAZY_DICT
            (192, 0x1833904), // LOAD_ATTR_NONDESCRIPTOR_WITH_VALUES
            (191, 0x19499c0), // LOAD_ATTR_NONDESCRIPTOR_NO_DICT
            (163, 0x180afc4), // CALL_NON_PY_GENERAL
            (155, 0x1837700), // CALL_KW_NON_PY
            (153, 0x1809700), // CALL_ISINSTANCE
            (25, 0x18023cc),  // NOP
        ],
    },
    // END VERIFIED BUILDS
];
