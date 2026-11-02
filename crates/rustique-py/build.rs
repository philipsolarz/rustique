//! Exposes PyO3's interpreter cfgs (`Py_GIL_DISABLED`, `Py_3_15`, …) to this crate: some
//! kernels differ between GIL and free-threaded builds.
//!
//! On Windows it also names the Python DLL (`rq_pydll = "python315"`, …) for
//! [`python_extern!`](src/lib.rs): PyO3 imports its own declarations with `raw-dylib`, and
//! the CPython symbols this crate declares itself need the same import.

fn main() {
    pyo3_build_config::use_pyo3_cfgs();
    println!(
        "cargo::rustc-check-cfg=cfg(rq_pydll, values(\"python314\", \"python314t\", \"python315\", \
         \"python315t\", \"python316\", \"python316t\"))"
    );
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Some(name) = pyo3_build_config::get().lib_name()
    {
        println!("cargo:rustc-cfg=rq_pydll=\"{name}\"");
    }
}
