//! The `RUSTIQUE*` environment switch (docs/ACCELERATION.md §8; semantics owned by the DX
//! layer, tests in `tests/test_dx*.py`): one implementation, here, for the startup path
//! (`_accel.startup()`, called by the `.pth` line after loading only this extension) and
//! for explicit `rustique.enable()` (`_accel.switch_enable(...)`).
//!
//! Variables (ignored under `-E`/`-I`, read at every call): `RUSTIQUE` (1/true/yes/on:
//! enable at startup; 0/false/no/off or empty: kill switch for every enable path, and the
//! `.pth` line loads nothing; anything else: a warning, nothing enabled), `RUSTIQUE_ONLY`, `RUSTIQUE_EXCLUDE`,
//! `RUSTIQUE_STATS`, `RUSTIQUE_CHECK`, `RUSTIQUE_REPORT`, `RUSTIQUE_TIERS` (opt-in tiers, in
//! union with `enable(tiers=...)`; `none`/`0`/empty turns every tier off even when code asks;
//! `RUSTIQUE_EXCLUDE` may name a tier). Warnings are single lines on `sys.stderr`; parsing
//! never raises.

use std::sync::Mutex;

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use super::{registry, stats};

const VARIABLES: [&str; 8] = [
    "RUSTIQUE",
    "RUSTIQUE_ONLY",
    "RUSTIQUE_EXCLUDE",
    "RUSTIQUE_STATS",
    "RUSTIQUE_REPORT",
    "RUSTIQUE_CHECK",
    "RUSTIQUE_TIERS",
    "RUSTIQUE_UNVALIDATED_PATCH",
];
const ON: [&str; 4] = ["1", "true", "yes", "on"];
const OFF: [&str; 4] = ["0", "false", "no", "off"];

#[derive(Default)]
struct State {
    /// The first enable path that ran (until `disable()`).
    source: Option<String>,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    /// Opt-in tiers in effect (union over enable paths since the last `disable()`).
    tiers: Vec<String>,
    killed_calls: u64,
    /// Where the report goes at exit (`"stderr"` or a path).
    report: Option<String>,
    atexit: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    source: None,
    only: None,
    exclude: Vec::new(),
    tiers: Vec::new(),
    killed_calls: 0,
    report: None,
    atexit: false,
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

/// `sys.flags.ignore_environment` (`-E`, `-I`; fixed for the life of the process).
fn ignore_environment(py: Python<'_>) -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| {
        // SAFETY: borrowed lookup in the sys module dict.
        let flags = unsafe { pyo3::ffi::PySys_GetObject(c"flags".as_ptr()) };
        if flags.is_null() {
            return false;
        }
        // SAFETY: a live borrowed object.
        let flags = unsafe { Bound::from_borrowed_ptr(py, flags) };
        flags.getattr("ignore_environment").and_then(|v| v.is_truthy()).unwrap_or(false)
    })
}

/// A `RUSTIQUE*` variable, or None (unset, or `-E`/`-I`). Read from the C environment,
/// which `os.environ` writes through (`putenv`/`unsetenv`).
fn env(py: Python<'_>, name: &str) -> Option<String> {
    if ignore_environment(py) {
        return None;
    }
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

/// A `RUSTIQUE*` variable set to an ON value (respecting `-E`/`-I`).
pub(crate) fn env_on(py: Python<'_>, name: &str) -> bool {
    env(py, name).is_some_and(|v| flag(&v) == Some(true))
}

/// `Some(true)` for ON values, `Some(false)` for OFF values, `None` otherwise.
fn flag(value: &str) -> Option<bool> {
    let v = value.trim().to_lowercase();
    if ON.contains(&v.as_str()) {
        Some(true)
    } else if OFF.contains(&v.as_str()) {
        Some(false)
    } else {
        None
    }
}

fn warn(py: Python<'_>, msg: &str) {
    if let Ok(err) = py.import("sys").and_then(|s| s.getattr("stderr")) {
        let _ = err.call_method1("write", (format!("{msg}\n"),));
    }
}

fn py_repr(py: Python<'_>, s: &str) -> String {
    pyo3::types::PyString::new(py, s).repr().map(|r| r.to_string()).unwrap_or_else(|_| format!("{s:?}"))
}

/// `"json, copy;re"` -> `["json", "copy", "re"]`: split on `,` and `;`, strip, drop empty,
/// dedup in first-seen order.
fn split_names(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in value.split([',', ';']) {
        let p = part.trim();
        if !p.is_empty() && !out.iter().any(|o| o == p) {
            out.push(p.to_owned());
        }
    }
    out
}

/// Names of a variable without the unknown ones (one warning per variable).
fn known(py: Python<'_>, var: &str, names: Vec<String>) -> Vec<String> {
    let specs = registry::all_specs();
    let is_known = |n: &str| registry::known_name(n);
    let unknown: Vec<&String> = names.iter().filter(|n| !is_known(n)).collect();
    if !unknown.is_empty() {
        let mut groups: Vec<&str> = specs.iter().map(|s| s.group).filter(|g| !g.starts_with('_')).collect();
        groups.sort_unstable();
        groups.dedup();
        let quoted: Vec<String> = unknown.iter().map(|n| py_repr(py, n)).collect();
        warn(
            py,
            &format!(
                "rustique: {var}: no target or group named {}, ignored (groups: {})",
                quoted.join(", "),
                groups.join(", ")
            ),
        );
    }
    names.into_iter().filter(|n| is_known(n)).collect()
}

/// What `RUSTIQUE_TIERS` asks for: `None` (unset: the code's tiers), `Some(None)` (`none`,
/// `0`, an OFF value or empty: no tier at all), `Some(Some(names))` (known names; unknown
/// ones warned about and dropped).
fn tiers_request(py: Python<'_>) -> Option<Option<Vec<String>>> {
    let raw = env(py, "RUSTIQUE_TIERS")?;
    let v = raw.trim().to_lowercase();
    if v.is_empty() || v == "none" || OFF.contains(&v.as_str()) {
        return Some(None);
    }
    let known_tiers = registry::tier_names();
    let names = split_names(&v);
    let unknown: Vec<String> =
        names.iter().filter(|n| !known_tiers.contains(&n.as_str())).map(|n| py_repr(py, n)).collect();
    if !unknown.is_empty() {
        warn(
            py,
            &format!(
                "rustique: RUSTIQUE_TIERS: no tier named {}, ignored (tiers: {})",
                unknown.join(", "),
                known_tiers.join(", ")
            ),
        );
    }
    Some(Some(names.into_iter().filter(|n| known_tiers.contains(&n.as_str())).collect()))
}

/// `RUSTIQUE_VALIDATE` (`eager` | `first-call`): the validation policy. Read by every
/// enable path (also `_accel.enable()`, once per process), so child processes of a CI job
/// inherit it.
pub(crate) fn apply_validate_env(py: Python<'_>) {
    if let Some(v) = env(py, "RUSTIQUE_VALIDATE") {
        match v.trim().to_lowercase().as_str() {
            "eager" => registry::store_eager(true),
            "first-call" | "" => registry::store_eager(false),
            _ => warn(
                py,
                &format!("rustique: ignoring RUSTIQUE_VALIDATE={} (expected eager or first-call)", py_repr(py, &v)),
            ),
        }
    }
}

/// `RUSTIQUE` set to an OFF value or empty: exactly like `0` (kill switch; the `.pth`
/// line loads nothing).
fn is_off(value: &str) -> bool {
    let v = value.trim().to_lowercase();
    v.is_empty() || OFF.contains(&v.as_str())
}

/// The kill switch: `RUSTIQUE` set to an OFF value or empty.
fn is_killed(py: Python<'_>) -> bool {
    env(py, "RUSTIQUE").is_some_and(|v| is_off(&v))
}

/// What `RUSTIQUE_CHECK` asks for: `Some("warn"|"raise"|"off")`, or None.
fn check_request(py: Python<'_>) -> Option<&'static str> {
    let raw = env(py, "RUSTIQUE_CHECK")?;
    let v = raw.trim().to_lowercase();
    if v.is_empty() {
        return None;
    }
    match v.as_str() {
        "warn" => return Some("warn"),
        "raise" => return Some("raise"),
        _ => {}
    }
    match flag(&v) {
        Some(true) => Some("warn"),
        Some(false) => Some("off"),
        None => {
            warn(py, &format!("rustique: ignoring RUSTIQUE_CHECK={} (expected 1, raise or 0)", py_repr(py, &raw)));
            None
        }
    }
}

/// Where `RUSTIQUE_REPORT` sends the report: None, `"stderr"` or a path (`{pid}`
/// replaced).
fn report_request(py: Python<'_>) -> Option<String> {
    let raw = env(py, "RUSTIQUE_REPORT")?;
    let v = raw.trim();
    if v.is_empty() {
        return None;
    }
    match flag(v) {
        Some(false) => None,
        Some(true) => Some("stderr".into()),
        None if v.eq_ignore_ascii_case("stderr") => Some("stderr".into()),
        None => Some(v.replace("{pid}", &std::process::id().to_string())),
    }
}

/// Register the report at exit (once per process) and set its target; `counters`: switch
/// counting on too.
fn register_report(py: Python<'_>, target: String, counters: bool) -> PyResult<()> {
    let first = {
        let mut st = state();
        st.report = Some(target);
        !std::mem::replace(&mut st.atexit, true)
    };
    if first {
        let accel = crate::module(py)?.getattr("_accel")?;
        py.import("atexit")?.call_method1("register", (accel.getattr("_report_at_exit")?,))?;
    }
    if counters {
        stats::set_enabled(true);
    }
    Ok(())
}

/// Names given from code (None, a str, or an iterable of str).
fn code_names(v: Option<&Bound<'_, PyAny>>) -> PyResult<Option<Vec<String>>> {
    let Some(v) = v else { return Ok(None) };
    if v.is_none() {
        return Ok(None);
    }
    let raw: Vec<String> = if let Ok(s) = v.extract::<String>() {
        vec![s]
    } else {
        v.try_iter()?
            .map(|x| {
                let x = x?;
                x.extract::<String>().map_err(|_| {
                    pyo3::exceptions::PyTypeError::new_err(format!(
                        "group or target names must be str, not {}",
                        x.get_type().name().map(|n| n.to_string()).unwrap_or_default()
                    ))
                })
            })
            .collect::<PyResult<_>>()?
    };
    let mut out: Vec<String> = Vec::new();
    for r in raw {
        for n in split_names(&r) {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    Ok(Some(out))
}

/// The env-aware quiet enable shared by every enable path (dx's steps 1–9).
pub fn switch_enable(
    py: Python<'_>,
    only: Option<Vec<String>>,
    exclude: Vec<String>,
    builtins: bool,
    source: &str,
    tiers: Option<Vec<String>>,
) -> PyResult<()> {
    // 1. Report registration (also when killed: the report says why nothing ran).
    let report = report_request(py);
    if let Some(r) = &report {
        register_report(py, r.clone(), false)?;
    }
    // 2. Kill switch.
    if is_killed(py) {
        state().killed_calls += 1;
        return Ok(());
    }
    // Names from code: unknown ones raise before anything changes.
    if let Some(o) = &only {
        registry::check_names(o)?;
    }
    registry::check_names(&exclude)?;
    if let Some(t) = &tiers {
        registry::check_tiers(t)?;
    }
    // Opt-in tiers: the code's in union with the environment's; the environment's `none`
    // turns every tier off.
    let sel_tiers: Vec<String> = match tiers_request(py) {
        Some(None) => Vec::new(),
        Some(Some(e)) => {
            let mut t = tiers.unwrap_or_default();
            for n in e {
                if !t.contains(&n) {
                    t.push(n);
                }
            }
            t
        }
        None => tiers.unwrap_or_default(),
    };
    // 3. Environment names (unknown ones dropped with a warning).
    let env_only = env(py, "RUSTIQUE_ONLY").map(|v| known(py, "RUSTIQUE_ONLY", split_names(&v)));
    let env_exclude =
        env(py, "RUSTIQUE_EXCLUDE").map(|v| known(py, "RUSTIQUE_EXCLUDE", split_names(&v))).unwrap_or_default();
    // 4. Narrowing: the environment narrows, never widens.
    let sel_only: Option<Vec<String>> = match (only, env_only) {
        (code, None) => code,
        (None, Some(e)) => Some(e),
        (Some(c), Some(e)) => Some(
            registry::all_specs()
                .iter()
                .filter(|s| {
                    (c.iter().any(|n| n == s.name || n == s.group)) && (e.iter().any(|n| n == s.name || n == s.group))
                })
                .map(|s| s.name.to_owned())
                .collect(),
        ),
    };
    let mut sel_exclude: Vec<String> = Vec::new();
    for n in exclude.into_iter().chain(env_exclude).chain((!builtins).then(|| "builtins".to_owned())) {
        if !sel_exclude.contains(&n) {
            sel_exclude.push(n);
        }
    }
    apply_validate_env(py);
    // 5. Check mode.
    match check_request(py) {
        Some("warn") => stats::set_check(Some(false)),
        Some("raise") => stats::set_check(Some(true)),
        Some(_) => stats::set_check(None),
        None => {}
    }
    // 6. Counters; with a `*.json` report, the live per-process counter file.
    if report.is_some() || env(py, "RUSTIQUE_STATS").is_some_and(|v| flag(&v) == Some(true)) {
        stats::set_enabled(true);
    }
    if let Some(r) = &report
        && r.ends_with(".json")
        && stats::enabled()
    {
        super::counters_file::start(py, r);
    }
    // 7. Install (quiet). An empty `only` (every RUSTIQUE_ONLY name unknown) installs
    // nothing.
    if sel_only.as_ref().is_none_or(|o| !o.is_empty()) {
        registry::enable_tiers(py, sel_only.clone(), sel_exclude.clone(), sel_tiers.clone())?;
    }
    // 8–9. Source (first path that ran) and the effective selection.
    let mut st = state();
    if st.source.is_none() {
        st.source = Some(source.to_owned());
    }
    st.only = sel_only;
    st.exclude = sel_exclude;
    for t in sel_tiers {
        if !st.tiers.contains(&t) {
            st.tiers.push(t);
        }
    }
    Ok(())
}

/// `disable()`: the next enable path records its source again.
pub fn reset_source() {
    let mut st = state();
    st.source = None;
    st.tiers.clear();
}

/// `process["switch"]` and `process["free_threaded"]` of `status()`.
pub fn describe(py: Python<'_>, process: &Bound<'_, PyDict>) -> PyResult<()> {
    let sw = PyDict::new(py);
    {
        let st = state();
        sw.set_item("source", st.source.as_deref())?;
        sw.set_item("only", st.only.as_ref().map(|o| PyTuple::new(py, o)).transpose()?)?;
        sw.set_item("exclude", PyTuple::new(py, &st.exclude)?)?;
        sw.set_item("tiers", PyTuple::new(py, &st.tiers)?)?;
        sw.set_item("killed_calls", st.killed_calls)?;
        sw.set_item("report", st.report.as_deref())?;
    }
    sw.set_item("killed", is_killed(py))?;
    sw.set_item("stats", stats::enabled())?;
    sw.set_item("check", stats::check_mode().map(|r| if r { "raise" } else { "warn" }))?;
    let envd = PyDict::new(py);
    for v in VARIABLES {
        if let Some(val) = env(py, v) {
            envd.set_item(v, val)?;
        }
    }
    sw.set_item("env", envd)?;
    process.set_item("switch", sw)?;
    let gil = py
        .import("sys")?
        .getattr("_is_gil_enabled")
        .and_then(|f| f.call0())
        .and_then(|v| v.is_truthy())
        .unwrap_or(true);
    process.set_item("free_threaded", !gil)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Python functions (registered on `_accel`)
// ---------------------------------------------------------------------------------------

/// The `RUSTIQUE=1` startup path (the `.pth` line, after loading only this extension).
/// Never raises: problems are reported on stderr and leave the interpreter untouched.
#[pyfunction]
pub fn startup(py: Python<'_>) {
    let Some(raw) = env(py, "RUSTIQUE") else { return };
    if is_off(&raw) {
        return;
    }
    let v = raw.trim().to_lowercase();
    if !ON.contains(&v.as_str()) {
        warn(py, &format!("rustique: ignoring RUSTIQUE={} (expected 1 or 0)", py_repr(py, &raw)));
        return;
    }
    if let Err(e) = switch_enable(py, None, Vec::new(), true, "RUSTIQUE=1", None) {
        let ty = e.get_type(py).name().map(|n| n.to_string()).unwrap_or_default();
        warn(py, &format!("rustique: RUSTIQUE={raw} ignored: {ty}: {}", e.value(py)));
    }
}

/// The env-aware enable of `rustique.enable()` & co. (quiet: returns None).
#[pyfunction]
#[pyo3(name = "switch_enable", signature = (only=None, exclude=None, builtins=true, source="rustique.enable()", tiers=None))]
pub fn py_switch_enable(
    py: Python<'_>,
    only: Option<&Bound<'_, PyAny>>,
    exclude: Option<&Bound<'_, PyAny>>,
    builtins: bool,
    source: &str,
    tiers: Option<&Bound<'_, PyAny>>,
) -> PyResult<()> {
    let only = code_names(only)?;
    let exclude = code_names(exclude)?.unwrap_or_default();
    let tiers = code_names(tiers)?;
    switch_enable(py, only, exclude, builtins, source, tiers)
}

/// The kill switch (`RUSTIQUE` set to an OFF value), live.
#[pyfunction]
pub fn killed(py: Python<'_>) -> bool {
    is_killed(py)
}

/// Report at exit to `target` (`"stderr"` or a path), counters on (`python -m rustique
/// --report`).
#[pyfunction]
pub fn report_to(py: Python<'_>, target: String) -> PyResult<()> {
    register_report(py, target, true)
}

/// This process's report record: `{"pid", "argv", "status", "stats", "counters": True}`.
#[pyfunction]
#[pyo3(name = "_record")]
pub fn record(py: Python<'_>) -> PyResult<Bound<'_, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("pid", std::process::id())?;
    let argv = py.import("sys")?.getattr("argv")?;
    let argv = PyList::new(py, argv.try_iter()?.collect::<PyResult<Vec<_>>>()?)?;
    d.set_item("argv", argv)?;
    d.set_item("status", super::py::status(py)?)?;
    d.set_item("stats", registry::stats(py, None, false)?)?;
    d.set_item("counters", true)?;
    Ok(d)
}

/// The atexit callback: `rustique._report.write(target, _record)` (formatting is imported
/// only now).
#[pyfunction]
#[pyo3(name = "_report_at_exit")]
pub fn report_at_exit(py: Python<'_>) -> PyResult<()> {
    // Written once: by the registry's exit hook (before it releases the installed state) or
    // by this callback, whichever runs first.
    let Some(target) = state().report.take() else { return Ok(()) };
    let accel = crate::module(py)?.getattr("_accel")?;
    py.import("rustique._report")?.call_method1("write", (target, accel.getattr("_record")?))?;
    super::counters_file::mark_json_written();
    Ok(())
}
