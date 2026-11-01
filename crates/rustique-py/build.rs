//! Exposes PyO3's interpreter cfgs (`Py_GIL_DISABLED`, `Py_3_15`, …) to this crate: the
//! locking discipline differs between GIL and free-threaded builds (see `src/sync.rs`).

fn main() {
    pyo3_build_config::use_pyo3_cfgs();
}
