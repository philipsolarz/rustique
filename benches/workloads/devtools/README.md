# devtools workloads

Developer tooling people run all day, in the shared workload format (`setup()`, `run(state)`,
`digest(result)`, `REQUIRES`). Pins: `requirements.txt` (CPython 3.15.1; all resolve on
3.15.1t too, but `pyyaml==6.0.3` has no cp315t wheel and builds from sdist). Inputs are the
running interpreter's own `Lib/` sources, test suites shipped inside wheels, or data
generated in `setup()`; the only bundled file is `data/pip_index.json` (44 kB).
Profiles and findings: `docs/atlas/workloads-devtools.md`.

| workload | stands for | run() on 3.15.1 |
|---|---|---|
| `pyflakes_stdlib` | pyflakes (flake8 F-codes) linting 61 stdlib modules, ~99k lines | ~1.2 s |
| `pycodestyle_stdlib` | pycodestyle (flake8 E/W) over 18 stdlib modules: tokenize + regex checks | ~1.1 s |
| `pylint_stdlib` | pylint, all default checkers, 3 stdlib modules, fresh astroid cache | ~1.1 s |
| `black_format` | black formatting 4 stdlib modules, safe mode (pure Python: no cp315 mypyc wheel) | ~1.3 s |
| `isort_stdlib` | isort over all 152 top-level stdlib modules (mypyc-compiled cp315 wheel) | ~0.3 s |
| `pytest_networkx` | `pytest.main()` on 554 tests of networkx's own suite (collection, fixtures, rewriting) | ~0.8 s |
| `sphinx_autodoc` | clean `sphinx-build -b html` of an autodoc API reference for 8 stdlib modules | ~0.9 s |
| `pip_resolve` | three offline `pip install --dry-run --report` resolutions over 1030 local wheels | ~0.6 s |
| `coverage_pyflakes_tests` | `coverage run --branch` (sysmon core) of pyflakes' 754 unit tests + `coverage report` | ~0.4 s |
| `sqlglot_transpile` | sqlglot parse + transpile to 3 dialects + optimizer on 100 generated queries | ~0.9 s |
| `rich_render` | rich rendering a table, markdown, syntax-highlighted source, tree, pretty to a string console | ~0.3 s |
| `import_startup` | 21 fresh `python -c "import X"` processes (bare + 20 popular packages); whole-process | ~2.0 s |

`profile_devtools.py` profiles one workload into the shared profile JSON (perf with
`-X perf` + BOLT-aware symbolization, cProfile, `sys.monitoring` argument sizes) and rolls a
family up; see its docstring. `import_startup` honours `DEVTOOLS_CHILD_PRELUDE` (code
prepended in every child, e.g. `import rustique; rustique.enable(); `).
