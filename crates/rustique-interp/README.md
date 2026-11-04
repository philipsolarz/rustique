# rustique-interp

The platform layer of Rustique's opt-in `interp` tier (docs/targets/interp.md): opcode
handlers generated from a CPython patch release's own `Python/bytecodes.c`, compiled with
clang into the extension, and swapped into the core interpreter's tail-call dispatch table on
verified python-build-standalone builds.

- `c/overrides.c`: the override file for CPython's cases generator (each `override op(...)`
  is CPython's original op with one helper wrapped); `c/overrides.lock` pins the original
  bodies the overrides were proven against.
- `c/rq_*.h`, `c/rq_handlers.c`: the overrides' fast paths and the handler-set interface.
- `gen/v<X>_<Y>_<Z>/`: generated sources for one release (`tools/gen.py`), with the CPython
  headers they read and the `pyconfig.h` of the verified builds, so a wheel build needs
  clang >= 19 but no CPython source. Each directory's `SOURCES.md` names the origin tag of
  every file.
- `src/builds.rs`: the verified builds (release, configuration, ELF build-id, table and
  handler addresses), recorded by `tools/record_build.py`.
- `src/lib.rs`: verification of the running binary, the ABI self-test, the set's interface.

Licences: the Rust and C code written for Rustique follows the repository's licence. The
generated files and the vendored CPython headers under `gen/`, and the parts of
`c/rq_handlers.c` and `c/overrides.c` adapted from CPython, are © Python Software Foundation
under the PSF License Agreement (`gen/LICENSE`, copied from CPython v3.15.1).
