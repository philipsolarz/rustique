//! Generic machinery for putting a Rust kernel behind an existing CPython entry point
//! (docs/atlas/mechanisms.md, draft contract for docs/ACCELERATION.md).
//!
//! Reusable by every target, independent of any particular kernel:
//! - [`site`]: patch sites (static or heap `PyMethodDef.ml_meth`, type slots, the
//!   `vectorcall` of a pure-Python function), all-or-nothing install, exact restore.
//! - [`guard`]: cheap per-call guards (tools listening, interpreter, watched dependencies).
//! - [`stats`]: per-target fast-path / fallback counters, free when nobody looks.
//! - [`version`]: build fingerprint used to refuse unknown interpreters per target.

// Wave-1 base: lifted from the Phase 1 prototype (branch p1/mechanisms); accel-core owns
// it from here. Unused until targets register.
#![allow(dead_code)]

pub mod guard;
pub mod site;
pub mod stats;
pub mod version;
