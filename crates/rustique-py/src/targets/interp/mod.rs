//! The `interp` tier (opt-in, docs/targets/interp.md, DECISIONS §10.2): opcode handlers
//! generated from the running CPython patch release's `Python/bytecodes.c`, written into the
//! core interpreter's tail-call dispatch table on verified python-build-standalone builds.
//!
//! Site kind: an **opcode handler** (one entry of `instruction_funcptr_handler_table`). The
//! platform layer (crate `rustique-interp`) verifies the release, configuration and ELF
//! build-id of the binary, that every entry holds the recorded core handler, and the
//! calling convention; each target here declares the entries of one override group, and the
//! registry swaps them all-or-nothing with compare-and-swap and restores them on `disable()`
//! and at exit. The handler code stays mapped for the life of the process.
//!
//! Targets (one per override group, so each can be excluded and reports its own state):
//! - `interp.isinstance`: CALL_ISINSTANCE (metaclass and negative fast paths);
//! - `interp.attr`: the LOAD_ATTR family (megamorphic attribute cache in the generic handler;
//!   the specialized forms are verbatim copies so their deopts reach it);
//! - `interp.setattr`: CALL_NON_PY_GENERAL (`object.__setattr__` direct);
//! - `interp.kwinit`: CALL_KW_NON_PY (keyword instantiation pushed inline; GIL builds only);
//! - `interp.getattr`, `interp.hasattr`: `ml_meth` kernels on the builtins that resolve
//!   through the same attribute cache (`getattr.rs`).

use std::sync::atomic::Ordering;

mod getattr;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use rustique_interp::{Group, Refuse};

use crate::accel::prepare::SiteDecl;
use crate::accel::site::{Kind, Site};
use crate::accel::{Category, Counters, Prepare, Refusal, TargetSpec};

/// The tier's group. ADAPTER until accel's opt-in tiers land (`TargetSpec.opt_in`): a hidden
/// group (`_`), selected only by name (`enable(only=["_interp"])`). Then: group `"interp"`,
/// `opt_in: Some("interp")`, enabled by `RUSTIQUE_TIERS=interp` / `enable(tiers=["interp"])`.
pub const GROUP: &str = "_interp";

pub(crate) const VERSIONS: &[(u8, u8)] = &[(3, 15)];

static C_ISINSTANCE: Counters = Counters::new();
static C_ATTR: Counters = Counters::new();
static C_SETATTR: Counters = Counters::new();
static C_KWINIT: Counters = Counters::new();
static C_GETATTR_GROUP: Counters = Counters::new();

fn counters(g: Group) -> &'static Counters {
    match g {
        Group::IsInstance => &C_ISINSTANCE,
        Group::Attr => &C_ATTR,
        Group::SetAttr => &C_SETATTR,
        Group::KwInit => &C_KWINIT,
        Group::GetAttr => &C_GETATTR_GROUP,
    }
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::IsInstance => "interp.isinstance",
        Group::Attr => "interp.attr",
        Group::SetAttr => "interp.setattr",
        Group::KwInit => "interp.kwinit",
        Group::GetAttr => "interp.getattr",
    }
}

pub(crate) fn refusal(r: &Refuse) -> Refusal {
    let cat = match r {
        Refuse::Platform(_) => Category::Platform,
        Refuse::Version(_) => Category::Version,
        Refuse::Build(_) => Category::Build,
        Refuse::Conflict(_) => Category::Conflict,
        Refuse::Selftest(_) => Category::Selftest,
        Refuse::NoHandlers(_) => Category::Other,
    };
    Refusal::with(cat, r.reason().to_owned())
}

/// A group suspended itself (its fast path did not pay for its handler copies): count it and
/// ask the registry to record the suspension (honoured once `resync` handles every site kind).
fn on_suspend(g: Group) {
    let c = counters(g);
    c.suspensions.fetch_add(1, Ordering::Relaxed);
    c.suspend_requested.store(true, Ordering::Release);
    crate::accel::registry::schedule_resync();
}

/// Declare the dispatch-table entries of one override group as sites.
fn declare(cx: &mut Prepare<'_>, group: Group) -> Result<(), Refusal> {
    let loc = rustique_interp::locate().map_err(|r| refusal(&r))?;
    rustique_interp::set_suspend_hook(on_suspend);
    let ops = loc.ops(group);
    if ops.is_empty() {
        return Err(Refusal::with(Category::Other, format!("no handlers for {}", group_name(group))));
    }
    let mut sites = Vec::with_capacity(ops.len());
    for &op in &ops {
        let (Some(core), Some(ours)) = (loc.core_handler(op), loc.handler(op)) else {
            return Err(Refusal::with(Category::Build, format!("{}: handler not recorded", loc.opname(op))));
        };
        // Every entry is compared with the expected core handler before the swap (the
        // registry's CAS compares again).
        if loc.current(op) != core {
            return Err(Refusal::with(
                Category::Conflict,
                format!("dispatch-table entry {} is not CPython's handler", loc.opname(op)),
            ));
        }
        sites.push(Site {
            name: format!("opcode {}", loc.opname(op)),
            // ADAPTER until accel's opcode-handler site kind lands: an entry of the dispatch
            // table is an aligned pointer cell swapped and restored by CAS, as a slot is.
            kind: Kind::Slot,
            addr: loc.cell(op),
            original: core,
            kernel: ours,
            installed: false,
        });
    }
    if !loc.arm(group, &ops) {
        return Err(Refusal::with(Category::Other, "cannot arm the override group"));
    }
    cx.sites.extend(sites.into_iter().map(|site| SiteDecl { site }));
    Ok(())
}

fn prepare_isinstance(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, Group::IsInstance)
}

fn prepare_attr(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, Group::Attr)
}

fn prepare_setattr(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, Group::SetAttr)
}

fn prepare_kwinit(_py: Python<'_>, cx: &mut Prepare<'_>) -> Result<(), Refusal> {
    declare(cx, Group::KwInit)
}

pub static ISINSTANCE: TargetSpec = TargetSpec {
    name: "interp.isinstance",
    group: GROUP,
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ISINSTANCE,
    prepare: prepare_isinstance,
    ..TargetSpec::DEFAULT
};

pub static ATTR: TargetSpec = TargetSpec {
    name: "interp.attr",
    group: GROUP,
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_ATTR,
    prepare: prepare_attr,
    ..TargetSpec::DEFAULT
};

pub static SETATTR: TargetSpec = TargetSpec {
    name: "interp.setattr",
    group: GROUP,
    module: None,
    versions: VERSIONS,
    free_threaded: true,
    counters: &C_SETATTR,
    prepare: prepare_setattr,
    ..TargetSpec::DEFAULT
};

/// GIL builds only: its cache holds `__init__` borrowed under the class's version tag, as the
/// specializer's own init cache does; on free-threaded builds that needs the specializer's
/// synchronization, which an extension cannot share (refused, category `build`).
pub static KWINIT: TargetSpec = TargetSpec {
    name: "interp.kwinit",
    group: GROUP,
    module: None,
    versions: VERSIONS,
    free_threaded: false,
    counters: &C_KWINIT,
    prepare: prepare_kwinit,
    ..TargetSpec::DEFAULT
};

pub static TARGETS: &[&TargetSpec] = &[&ISINSTANCE, &ATTR, &SETATTR, &KWINIT, &getattr::GETATTR, &getattr::HASATTR];

/// `_accel._t_interp.info()`: where the tier stands in this process (for tests, benches and
/// status tooling).
#[pyfunction]
fn info(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("compiled", PyList::new(py, rustique_interp::compiled_releases())?)?;
    match rustique_interp::locate() {
        Ok(loc) => {
            d.set_item("verified", true)?;
            d.set_item("release", loc.release())?;
            d.set_item("build_id", loc.build.build_id)?;
            d.set_item("source", loc.build.source)?;
            d.set_item("object", &loc.object)?;
            let groups = PyDict::new(py);
            for g in rustique_interp::GROUPS {
                let s = loc.stats(g);
                let gd = PyDict::new(py);
                gd.set_item("executions", s.executions)?;
                gd.set_item("hits", s.hits)?;
                gd.set_item("fills", s.fills)?;
                gd.set_item("suspended", s.suspended)?;
                gd.set_item("windows", s.windows)?;
                let swapped: Vec<&str> = loc
                    .ops(g)
                    .into_iter()
                    .filter(|&op| Some(loc.current(op)) == loc.handler(op))
                    .map(|op| loc.opname(op))
                    .collect();
                gd.set_item("swapped", swapped)?;
                groups.set_item(group_name(g), gd)?;
            }
            d.set_item("groups", groups)?;
        }
        Err(r) => {
            d.set_item("verified", false)?;
            d.set_item("reason", r.reason())?;
            d.set_item("category", refusal(&r).category().as_str())?;
        }
    }
    Ok(d)
}

pub fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let sub = PyModule::new(m.py(), "_t_interp")?;
    sub.add_function(wrap_pyfunction!(info, &sub)?)?;
    m.add("_t_interp", sub)
}
