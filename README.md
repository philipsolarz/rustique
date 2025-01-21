RUSTFLAGS="--cfg Py_3_13" maturin develop --uv --release


1. Add slicing to list
2. Add nested lists and nested typings


cargo.toml

[profile.release]
lto = "fat"
codegen-units = 1



For dict constructor
implement PyMapping
investigate Sequence (Rust side)
investigat Mapping (Rust side advanced)

It might be that we only need to check
PyMapping, PySequence, PyIterator
RustMapping, RustSequence, RustIterator