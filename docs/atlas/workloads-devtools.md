# Atlas: where developer tools and CLIs spend their time (family `devtools`)

Round question 1 (*where does real Python spend its time?*) for the tools developers run all
day: linters, formatters, test runners, doc builders, the pip resolver, coverage, SQL tooling,
terminal rendering, and process startup. 12 workloads (`benches/workloads/devtools/`), one
profile each in `~/rustique-research/profiles/devtools/<workload>.json` (shared schema +
extensions, §6), plus a pure-Python isort variant. CPython 3.15.1 (PBS, PGO+LTO+BOLT),
shared 8-CPU WSL2 box, 2026-11-01. Every number is **measured** unless tagged *inferred*.

## Key findings

1. **~69 % of devtools CPU time is plainly unreachable through existing entry points**
   (mean of 11 steady-state workloads, perf): bytecode execution of the tools' own Python
   36.8 %, native work done *inside* opcodes (attribute lookup, dict/list ops, iteration)
   ~12.8 %, the cyclic GC 10.9 %, builtins the specializer inlines (`CALL_ISINSTANCE`,
   `CALL_LEN`, `CALL_LIST_APPEND`, `CALL_BUILTIN_CLASS`…) 3.0 %, class instantiation 1.3 %,
   mypyc-compiled code 4.4 %. Reachable: **16.9 % under pure-Python stdlib functions** and
   **10.1 % under builtin C entry points** that go through `PyMethodDef.ml_meth`/vectorcall,
   plus ~1.7 % under iterator slots (`finditer`, `tokenize`). Table §2.1.
2. **The cyclic GC is the biggest single native cost**: 10.9 % mean, **30–34 %** in pylint and
   pytest, 17 % in pyflakes (`gc_collect_main`, `visit_decref`, `visit_reachable`,
   `dict_traverse`). Component `memory/gc` (GC + allocator + dealloc) is 20.8 % of all samples,
   second only to the interpreter (41.4 %). Not an entry point Rustique can swap.
3. **String builtins are not where devtools spend time**: component `str` is 2.8 % mean
   (max 10 % in mypyc isort), `re-engine` 3.0 % (but 11.6 % sphinx, 13.2 % pycodestyle).
   The largest *reachable* C entries are `compile()` behind `ast.parse` (3.5 % mean; 20.9 %
   pyflakes), `re.Pattern.match` (2.8 % mean; 20.6 % sphinx via pygments: **2.49 M calls per
   run**) and the mypyc isort extension (4.4 %).
4. **Biggest stdlib-function targets**: `dataclasses.asdict` → `copy.deepcopy` (**44 %** of
   isort, 29 % of pure-Python isort, `deepcopy` 5.1 % of sqlglot); `ast.parse` (21 % pyflakes,
   8.6 % coverage, 5.4 % pylint); the `tokenize` generator + `TokenInfo._make` (6.6 %
   pycodestyle, 183 k tokens/run); pip's `zipfile` + `email.parser` + `importlib.metadata` +
   `pathlib` (together ~25 % of `pip_resolve`); `functools.cached_property.__get__` (4.3 %
   pylint); `contextlib.contextmanager` (3.6 % pyflakes). Ranked list §3.
5. **Startup** (21 fresh processes, ~94 ms each): finalization **16 %** of main-thread CPU,
   unmarshalling `.pyc` **13.7 %** (`marshal.loads`, p50 11 kB), importlib bootstrap
   15.8 % (Python 7.2 % + native helpers 8.6 %), `typing` 6.8 %, `re.compile` at import 5.1 %, enum/dataclass/ABC/namedtuple
   class creation 9.6 %, GC 5.8 %; kernel time (stat/open/read/mmap) 12–15 % of CPU
   (rusage, not sampled). 47 % of `-X importtime` self time is in *stdlib* modules. §4.
6. **Compiled tools**: isort 9 ships mypyc cp315 wheels (1.4× faster than its pure build here)
   and still spends **47 %** in the pure-Python stdlib (`asdict`/`deepcopy`); black has **no**
   cp315 mypyc wheel, so 3.15 users run it pure (59 % bytecode). §5.

Decisions needed are in §7.

## 1. Workloads

| workload | stands for | run() (plain, median of 5) | notes |
|---|---|---|---|
| `pyflakes_stdlib` | pyflakes over 61 stdlib modules (99 k lines) | 1.18 s | flake8 F-codes |
| `pycodestyle_stdlib` | pycodestyle over 18 stdlib modules | 1.14 s | tokenize + ~70 regex/str checks per line |
| `pylint_stdlib` | pylint, default checkers, 3 stdlib modules, astroid cache cleared per run | 1.13 s | |
| `black_format` | black, safe mode, 4 stdlib modules (2.3 k lines) | 1.28 s | pure Python on 3.15 (no cp315 wheel) |
| `isort_stdlib` | isort over the 152 top-level stdlib modules | 0.29 s | mypyc cp315 wheel; pure build 0.41 s |
| `pytest_networkx` | `pytest.main()` on 554 networkx tests | 0.79 s | test modules re-imported per run |
| `sphinx_autodoc` | clean HTML build, autodoc + viewcode of 8 stdlib modules | 0.86 s | docutils, Jinja2, Pygments |
| `pip_resolve` | 3 offline `pip install --dry-run --report` resolutions, 1030 wheels | 0.61 s | real metadata of 103 dists, backtracking |
| `coverage_pyflakes_tests` | `coverage run --branch` (sysmon core) of 754 pyflakes tests + report | 0.40 s | |
| `sqlglot_transpile` | parse + 3-dialect transpile + optimizer, 100 queries | 0.89 s | pure Python (no sqlglotrs) |
| `rich_render` | table, markdown, highlighted source, tree, pretty → string console | 0.32 s | |
| `import_startup` | 21 × `python -c "import X"` (bare + 20 popular packages) | 1.97 s | whole-process |

Pins: `benches/workloads/devtools/requirements.txt` (32 pins). On 3.15.1t every pin resolves
(`uv pip install --dry-run`); only `pyyaml==6.0.3` lacks a cp315t wheel (sdist build; used by
`import_startup` only). Not profiled on 3.15.1t this round. `mypy` was not made a workload:
it is mypyc-compiled (cp315 wheel, 106 extension modules); isort is the compiled-tool case.

## 2. Family roll-up (11 steady-state workloads; startup in §4)

### 2.1 Reachability per workload (share of perf samples)

Classes are exclusive per sample (definitions in §6). *Interpreter* = leaf in the eval loop
for tool code; *opcode-internal* = native work called by a non-CALL opcode handler
(`LOAD_ATTR` → `PyObject_GenericGetAttr`, dict subscripts, `FOR_ITER`…); *builtin via ml_meth*
= a CALL-family opcode reached a builtin's `PyMethodDef.ml_meth`/vectorcall (swappable the
way `rustique.enable()` works today); *builtin inlined* = the specialized opcode did the
builtin's work itself (`CALL_ISINSTANCE` → `PyObject_IsInstance`, `CALL_LEN`,
`CALL_LIST_APPEND`, `CALL_TYPE_1/STR_1/TUPLE_1`, `CALL_BUILTIN_CLASS`; Python/bytecodes.c);
*stdlib fn* = innermost Python frame is stdlib (incl. native below it).

| workload | -X perf × | kernel | interpreter | opcode-internal | builtin via ml_meth | builtin inlined | class call | stdlib fn | GC | C-ext/mypyc | artifact |
|---|---|---|---|---|---|---|---|---|---|---|---|
| pyflakes_stdlib | 1.08 | 1.8% | 33.3% | 14.4% | 3.0% | 3.7% | 0.4% | 26.6% | 16.7% | 0.0% | 1.9% |
| pycodestyle_stdlib | 1.11 | 0.0% | 51.8% | 26.9% | 11.9% | 0.5% | 0.1% | 7.1% | 0.0% | 0.0% | 1.7% |
| pylint_stdlib | 1.09 | 1.7% | 35.4% | 14.5% | 4.5% | 1.8% | 1.7% | 9.7% | 30.4% | 0.0% | 2.1% |
| black_format | 1.14 | 0.1% | 59.1% | 15.7% | 7.0% | 1.9% | 1.8% | 3.1% | 9.0% | 0.0% | 2.4% |
| isort_stdlib | 1.07 | 0.6% | 1.1% | 0.1% | 2.2% | 0.0% | 0.0% | 46.8% | 0.1% | 48.2% | 1.4% |
| pytest_networkx | 1.00 | 1.7% | 27.8% | 10.1% | 8.1% | 3.7% | 0.9% | 13.9% | 33.8% | 0.0% | 1.3% |
| sphinx_autodoc | 1.08 | 2.4% | 34.5% | 11.3% | 32.9% | 1.7% | 0.5% | 10.4% | 6.9% | 0.0% | 1.8% |
| pip_resolve | 1.05 | 4.5% | 28.3% | 13.7% | 8.6% | 5.2% | 1.6% | 37.6% | 2.1% | 0.0% | 3.0% |
| coverage_pyflakes_tests | 1.15 | 2.0% | 31.4% | 17.2% | 7.0% | 4.2% | 2.5% | 23.2% | 12.5% | 0.0% | 1.8% |
| sqlglot_transpile | 1.12 | 0.0% | 52.2% | 16.4% | 5.4% | 8.6% | 1.9% | 6.2% | 7.1% | 0.0% | 2.1% |
| rich_render | 1.15 | 0.0% | 50.3% | 19.1% | 20.9% | 1.7% | 2.8% | 1.0% | 1.7% | 0.0% | 2.5% |
| **mean** | | | **36.8%** | **14.5%** | **10.1%** | **3.0%** | **1.3%** | **16.9%** | **10.9%** | **4.4%** | **2.0%** |
| isort pure-Python build | 1.09 | 0.0% | 43.0% | 15.7% | 6.7% | 0.2% | 0.7% | 32.2% | 0.0% | 0.0% | 1.4% |

Of the 16.9 % stdlib share, about half (9.4 % mean) is the stdlib's own bytecode
(interpreter/attr/dict/calls leaves under stdlib frames) and half (9.7 %) native code below
stdlib functions (the parser under `ast.parse`, GC/allocation triggered there). The
`-X perf` column is run() wall under `-X perf` ÷ plain (noisy box; earlier runs gave up to
×1.23): PEP 523 disables inlined Python→Python calls, so *interpreter* is slightly inflated.

### 2.2 Top 15 (owner × leaf component) cells, mean share

| # | owner × component | mean | max in one workload |
|---|---|---|---|
| 1 | thirdparty:pyflakes × interpreter | 5.2% | 31.3% (pyflakes) |
| 2 | thirdparty:pycodestyle × interpreter | 4.5% | 49.5% (pycodestyle) |
| 3 | thirdparty:sqlglot × interpreter | 4.4% | 48.5% (sqlglot) |
| 4 | thirdparty:black × interpreter | 4.1% | 44.6% (black) |
| 5 | thirdparty:rich × interpreter | 3.8% | 41.7% (rich) |
| 6 | stdlib:ast × memory/gc | 2.7% | 15.9% (pyflakes) |
| 7 | thirdparty:astroid × interpreter | 2.5% | 27.3% (pylint) |
| 8 | thirdparty:pip × interpreter | 2.4% | 26.2% (pip) |
| 9 | thirdparty:networkx × memory/gc | 2.1% | 22.9% (pytest) |
| 10 | thirdparty:pyflakes × memory/gc | 1.9% | 12.0% (coverage) |
| 11 | stdlib:ast × parser | 1.5% | 9.4% (pyflakes) |
| 12 | stdlib:copy × interpreter | 1.5% | 14.3% (isort) |
| 13 | thirdparty:astroid × memory/gc | 1.5% | 16.8% (pylint) |
| 14 | thirdparty:pyflakes × attr/type | 1.4% | 8.7% (pyflakes) |
| 15 | thirdparty:black × memory/gc | 1.4% | 15.2% (black) |

Leaf components over all 11: interpreter 41.4 %, memory/gc 20.8 %, attr/type 7.6 %, dict
4.4 %, calls 4.1 %, list/tuple/set 3.3 %, re-engine 3.0 %, libc 2.8 %, str 2.8 %,
profiler-artifact 2.0 %, parser 1.9 %. Hottest interpreter leaves: `LOAD_ATTR*` family,
`CALL`, `RETURN_VALUE`, `LOAD_GLOBAL_*`, `CALL_ISINSTANCE`, `STORE_FAST`.

### 2.3 Builtin methods, slots and C functions (native entry incl. children, mean share)

| native entry | called from | mean | reachable through |
|---|---|---|---|
| cyclic GC (`gc_collect_main` from eval breaker/allocation) | – | 10.95% | none |
| isort's mypyc extension | `CALL_KW_NON_PY` | 4.36% | none (third-party native) |
| `builtin_compile` (mostly `ast.parse`) | `CALL_KW_NON_PY` | 3.54% | `compile` ml_meth, or `ast.parse` |
| `PyObject_GenericGetAttr` | `LOAD_ATTR` | 2.97% | interpreter (tp_getattro of every object) |
| `_sre_SRE_Pattern_prefixmatch` (`Pattern.match`) | `CALL_NON_PY_GENERAL` | 2.79% | ml_meth (`builtin_method`, bound) |
| `_PyFunction_Vectorcall` | `CALL` | 1.89% | none (Python call setup; inflated by `-X perf`) |
| `object_recursive_isinstance` | `CALL_ISINSTANCE` | 1.64% | none (inlined by the opcode) |
| `calliter_iternext` (`Pattern.finditer` iteration) | `FOR_ITER` | 1.21% | `SRE_Scanner.search` ml_meth |
| `type_call` (class instantiation) | `CALL*` | 1.68% | tp_new/tp_init slots (mostly Python classes) |
| `builtin_getattr` | `CALL_BUILTIN_FAST` | 0.72% | ml_meth |
| `PyObject_GenericSetAttr`, `_PyObject_GetMethodStackRef` | `STORE_ATTR`/`LOAD_ATTR` | 1.29% | interpreter |
| `bounded_lru_cache_wrapper` (`functools.lru_cache`) | `CALL_NON_PY_GENERAL` | 0.59% | tp_call of the C wrapper |
| `_PyDict_Subscript`, `_PyDict_SetItem_Take2` | `BINARY_OP_SUBSCR_DICT`, `STORE_SUBSCR_DICT`, `MAP_ADD` | 1.42% | interpreter (specialized dict ops) |
| `tokenizeriter_next` (C tokenizer behind `tokenize`) | `FOR_ITER` | 0.52% | tp_iternext, or `tokenize.generate_tokens` |
| `reversed_new_impl`, `make_new_set` | `CALL_BUILTIN_CLASS` | 0.76% | type vectorcall (not ml_meth) |
| `_sre_SRE_Pattern_search` | `CALL_NON_PY_GENERAL` | 0.33% | ml_meth |
| `frozenset_vectorcall`, `object___reduce_ex__`, `tp_new_wrapper` | CALL | 0.94% | ml_meth/vectorcall (mostly under `deepcopy`) |

cProfile view (per run, all 11 summed; tottime share is cProfile-inflated for tiny calls):
`isinstance` **11.7 M calls**, mean 4.6 % of cProfile time; `Pattern.match` 3.0 M, 2.1 %;
`getattr` 3.1 M, 1.4 %; `len` 3.7 M, 1.0 %; `list.append` 2.6 M, 1.2 %; `dict.get` 1.9 M,
1.2 %; `str.lower` 1.1 M, 0.3 %; `str.join` 0.24 M, 0.3 %; `compile` 2,312 calls, 2.3 %.
`isinstance`, `len`, `list.append` are mostly executed by the specialized opcodes, so a
`PyMethodDef` swap would not see them (measured: in all 13 profiles every
`object_recursive_isinstance` entry sample was called from `CALL_ISINSTANCE`).

### 2.4 Argument sizes at the hottest entries (`sys.monitoring`, one run; measured)

| hotspot | workload | calls/run | p50 | p90 | p99 | max | note |
|---|---|---|---|---|---|---|---|
| `compile` (via `ast.parse`) source chars | pyflakes | 62 | 51,066 | 110,615 | 229,714 | 229,714 | whole files |
| `compile` source chars | coverage | 1,850 | 40 | 135 | 1,504 | 78,970 | test snippets |
| `re.Pattern.match` subject chars | sphinx (pygments) | 2,485,719 | 41,448 | 114,307 | 114,307 | 114,307 | whole module + `pos`; per-call overhead dominates |
| `re.Pattern.match` subject chars | pycodestyle | 183,664 | 27 | 69 | 233 | 6,503 | logical lines |
| `re.Pattern.finditer` / `.search` subject | pycodestyle | 88,238 / 44,150 | 22 | 61 | 388 | 6,503 | |
| `re.Pattern.match` subject chars | rich | 249,833 | 38,765 | 38,765 | 38,765 | 38,765 | pygments, one text |
| `str.lower` self chars | black / pip | 721,598 / 285,504 | 3 / 13 | 3 / 16 | 3 / 29 | 36 / 293 | tiny strings |
| `str.startswith` self chars | isort (pure) | 623,948 | 24 | 59 | 76 | 3,512 | source lines |
| `str.split` self chars | pip | 87,725 | 8 | 39 | 66 | 185 | |
| `len(x)` (x = str/list/tuple) | black | 2,221,695 | 3 | 35,851 | 35,949 | 35,949 | `CALL_LEN` inlined |
| `dict.get` dict size | sqlglot | 803,933 | 2 | 37 | 779 | 779 | |
| `copy.deepcopy` (first sized arg) | isort | 429,510 | 7 | 12 | 19 | 367 | str/frozenset/list/enum leaves |
| `dataclasses._asdict_inner` | isort | 170,465 | 9 | 20 | 103 | 367 | config object per file |
| `namedtuple._make` (TokenInfo) | pycodestyle | 183,255 | 5 | 5 | 5 | 5 | one per token |
| `isinstance` | sqlglot / pyflakes | 3,438,890 / 2,954,055 | – | – | – | – | types: `Identifier`, `str`, `FunctionScope`, `Name` |

All sizes per call, chars for `str`, elements for containers. Raw tables (incl. types and
histograms): `arg_sizes` in each profile JSON.

## 3. Accelerable behind an existing entry point vs. interpreter work

Ranked by measured share × breadth. Ceilings are *inferred* (share × assumed speedup).

| target (entry point) | kind | measured share | ceiling (*inferred*) | notes |
|---|---|---|---|---|
| `copy.deepcopy` (+ `dataclasses.asdict`, `copyreg`) | pure-Python stdlib | isort 44 % (compiled) / 29 % (pure); sqlglot 5.1 % | isort −34 % wall at the 0.22× third-party upper bound | leaves are small (p50 7 elements); exact `__deepcopy__`/`__reduce_ex__`/memo protocol needed |
| `ast.parse` → `compile(..., PyCF_ONLY_AST)` | stdlib fn over C parser + `ast2obj` | pyflakes 21 %, coverage 8.6 %, pylint 5.4 %, sphinx 1.3 %, black 1.0 % | pyflakes −10 % at 2× | ~45 % parser, rest AST object allocation + GC it triggers; parser is an upstream 3.17 Rust candidate (overlap) |
| `re.Pattern.match/search/finditer` (`_sre`) | C builtin methods (ml_meth) | sphinx 20.6 % + finditer; pycodestyle 13.2 % re-engine; rich 6.1 % | sphinx −10 % at 2× | 2.49 M calls/run in pygments, subjects 41–114 kB with `pos`: per-call setup matters (150–174 ns/call measured); exact sre semantics required |
| `tokenize.generate_tokens` (+ `TokenInfo._make`) | stdlib generator over C tokenizer | pycodestyle 6.6 %, coverage 2.8 %, pylint (tokens via `_make`, 7.3 % self incl. GC) | pycodestyle −4 % | one namedtuple per token; a C/Rust path that builds the exact `TokenInfo` type |
| pip's metadata path: `zipfile.ZipFile` (`__init__`, `read`), `email.parser`, `importlib.metadata.Distribution.*`, `pathlib.PurePath.__hash__/relative_to` | pure-Python stdlib | pip 37.6 % stdlib in total (top entries 5.6/4.7/3.0/2.9/2.5 %) | pip −15–20 % at 2× | mostly Python-level bytecode inside the stdlib (23 % of pip) |
| `functools.cached_property.__get__` | pure-Python stdlib | pylint 4.3 % self | pylint −3 % | astroid hits it on every node property |
| `contextlib.contextmanager` + `functools.update_wrapper/wraps` | pure-Python stdlib | pyflakes 3.6 % + 2.1 %, coverage 3.0 % + 0.7 % | ~−3 % | per-node context managers/decorators |
| `inspect.signature`, `_py_warnings.filterwarnings`, `logging` record path | pure-Python stdlib | pytest 2.0 % / 1.4 %, sphinx 3.1 % | ≤ −2 % each | |
| `namedtuple` generated `__new__` (`<string>:<lambda>`) | stdlib-generated code | rich 3.5 % (`Segment`) | rich −2 % | owner `generated`; a C `__new__` for namedtuple classes |
| `compile`/`exec`/`marshal.loads`/importlib (startup) | stdlib + builtins | see §4 | see §4 | |

Not reachable without hooking the interpreter or changing semantics (measured means):
tool bytecode 36.8 %, opcode-internal attribute/dict work ~12.8 %, GC 10.9 %, inlined
builtins 3.0 %, class calls 1.3 %, mypyc code 4.4 % — **≈ 69 %**. Even with every reachable
target made free, a typical devtools run would get ~1.4× faster (*inferred*: 1 / (1 − 0.29));
realistic 2× kernels on the reachable part give ~10–15 %, concentrated in isort, pyflakes,
sphinx, pip and coverage. GC alone (10.9 %, up to 34 %) is larger than any single target.

## 4. Startup and import (`import_startup`, whole-process)

21 processes per run, 1.97 s (≈ 94 ms per process; bare `python -c pass` ≈ 15 ms). perf
over the children (64 processes in 3 runs): **37 % of all samples were OpenBLAS worker
threads** spinning after `import numpy`/`pandas` (extra CPU, not wall); they are excluded
below, which is main-thread attribution (9,426 samples). Kernel time 12–15 % of CPU
(rusage: stat/open/read/mmap and page faults, not sampled by `cpu-clock:u`).

| category (main thread, exclusive, priority order) | share | how attributed |
|---|---|---|
| interpreter **finalization** (module teardown, final GC, deallocs) | 16.0% | no Python frame, `Py_FinalizeEx`/`finalize_modules` on stack |
| interpreter initialization | 4.0% | `pyinit_*`/`init_interp_main` |
| unmarshal `.pyc` (`marshal.loads` from `_compile_bytecode`) | 13.7% | `marshal_loads`/`r_object` below |
| importlib bootstrap, Python code (`_find_and_load`, `FileFinder.find_spec`, `_path_join`, locks) | 7.2% | innermost frame `<frozen importlib._bootstrap*>`, leaf interpreter |
| importlib native helpers (`_imp.exec_builtin`, `get_frozen_object`, dict/attr ops) | 8.6% | same owner, other native entry |
| importlib file I/O: `open_code`/`read` + `stat`/`listdir` (user side only) | 1.7% | `_io_*`, `os_stat` entries |
| `typing` / `annotationlib` machinery (incl. `compile` of `ForwardRef` strings) | 6.8% | `typing.py`/`annotationlib.py` frame anywhere on stack |
| `re.compile` at import (`re._parser`, `re._compiler`) | 5.1% | |
| enum / dataclass / ABC / namedtuple class creation | 3.4 / 3.3 / 1.6 / 1.3% | |
| other class creation (`__build_class__` → `type_new`) | 4.8% | |
| `inspect` (signatures at import) | 1.6% | |
| `exec`/`compile`/`eval` of generated code | 1.7% | |
| cyclic GC during import | 5.8% | |
| third-party / other stdlib module-level code | 7.2 / 4.1% | |
| C-extension module init (`_imp.create_dynamic`/`exec_dynamic`) | 0.7% | |

Per-call sizes (sys.monitoring in every child; per run of 21 processes): `marshal.loads`
2,679 calls, p50 11 kB, p90 72 kB, max 695 kB; `posix.stat` 13,592 calls (≈ 650 per
process), path p50 94 chars; `re._compiler._compile` 5,977; `compile` 2,512 calls with source
p50 **12 chars** (annotation strings); `exec` 3,235, generated source p50 432 chars;
`__build_class__` 10,359 classes. `-X importtime` (median of 3): summed self time 1,686 ms
over the 21 imports, **47 % in stdlib modules**; largest stdlib totals `typing` 38.6 ms,
`inspect` 29.5, `enum` 19.4, `_ast` 18.0, `site` 17.4, `logging` 17.0, `collections` 14.0,
`dis` 12.1, `ssl` 11.2, `re._parser` 10.3. Largest third-party: `fastapi.openapi.models`
35 ms (pydantic model build), `black.nodes` 21 ms.

Startup targets behind entry points: `marshal.loads` (C function, 13.7 %; Rust unmarshal must
reproduce code objects exactly — *inferred* hard but well-specified), `re.compile`'s
pure-Python compiler (5.1 %), `typing`/`enum`/`dataclasses` class construction (pure Python,
~15 % together), importlib's frozen Python (7–9 %; replacing frozen bootstrap code is
intrusive). Finalization (16 %) and init (4 %) are interpreter work.

## 5. mypyc-compiled tools

| | isort compiled (cp315 wheel) | isort pure (`--no-binary`) |
|---|---|---|
| run() | 0.29 s | 0.41 s (1.4× slower) |
| mypyc/C-extension code (owner `thirdparty:isort (native)`) | 48.3 % (leaf in the extension 13.7 %, str C-API 9.3 %, alloc/GC 7.1 %, attr 4.8 %, libc 3.1 %, dict 2.3 %, list 1.9 %) | – |
| stdlib Python (`dataclasses.asdict` → `copy.deepcopy`) | 46.8 % | 32.2 % |

mypyc code calls the C API directly (`PyUnicode_*`, `PyDict_*`), so method-table swaps never
see those calls (measured: 13.5 % of isort samples are str/dict/list C-API leaves under the
extension, plus 7.1 % allocation). What remains reachable in compiled tools is the stdlib they call into — here the
largest single target in the family. black: PyPI has no cp315 mypyc wheel for 26.10.1
(`py3-none-any` only), so 3.15 users run it pure (59 % bytecode, 9 % GC); on 3.12–3.14 it is
compiled (*inferred* from the wheel tags).

## 6. Method and reproduction

```
uv venv --python 3.15 ~/rustique-research/w-devtools/venv
VIRTUAL_ENV=~/rustique-research/w-devtools/venv uv pip install -r benches/workloads/devtools/requirements.txt
python3.15 benches/workloads/devtools/profile_devtools.py profile benches/workloads/devtools/<w>.py \
    --python ~/rustique-research/w-devtools/venv/bin/python \
    --out ~/rustique-research/profiles/devtools/<w>.json [--whole-process]   # ~30 s each
python3.15 benches/workloads/devtools/profile_devtools.py rollup ~/rustique-research/profiles/devtools/*.json
```

| pass | what | details |
|---|---|---|
| timing | 5 plain `run()` after setup + 1 warm-up | `wall_s` = median; user/sys from `getrusage` (self + children) |
| perf | `perf record -e cpu-clock -F 1999 -g`, child with `PYTHONPERFSUPPORT=1` | events enabled only around ≥ 5 s of steady-state `run()` (`--control` fifo); 10–12 k main-thread samples per workload |
| cProfile | separate process, `run()` repeated ≥ 3 s | per-run calls / tottime; builtins = cProfile's `~` rows |
| arg sizes | separate process, one `run()` under `sys.monitoring` | CALL for the top 8 cProfile builtins (callable identity incl. 3.15's `builtin_method` type for bound C methods), PY_START for the top 6 stdlib functions; sizes only of exact builtin containers |
| importtime | `-X importtime`, 3 repeats, median per module | `import_startup` only |

**Symbolization (BOLT)**: perf 6.18 here reports leaf and callchain IPs of the PBS binary as
*virtual addresses*; the brief's file-offset correction is wrong on this machine and was not
used. perf cannot name the BOLTed code, so `profile_devtools.py` maps raw vaddrs with
`nm -nS` over every executable section, including `.bolt.org.text` (BOLT leaves un-moved
functions such as `py_trampoline_evaluator` executing there; ~3 % of samples). Validated on
`json.dumps` of a 20 k-element list: 100 % of the 5,995 samples under `iterencode` have
`encoder_call`/`encoder_listencode_*` on the stack (top leaves `_Py_dg_dtoa`,
`encoder_listencode_obj`); with the offset added, leaves become `_PyInterpreterConfig_AsDict`,
`sqlite3WindowCodeStep`. Unsymbolized leaves ≤ 7 per profile (of ~10 k).

**Definitions.** *Owner* = file of the innermost Python frame (`stdlib:<top-level module>`,
frozen importlib = `stdlib:importlib`, `thirdparty:<distribution>` via
`packages_distributions()`, `generated` = `<string>` code such as namedtuple `__new__`,
`workload`, `<no-python-frame>`); C-extension code between the leaf and that frame makes it
`thirdparty:<dist> (native)` (.so → distribution via RECORD). *Component* = leaf symbol by
prefix table (brief's, plus `monitoring`, `parser`, `ast-objects`, `compiler`, `marshal`,
`isinstance/abc`, `builtins`, `iter/gen`, `functools`, `collections`, `operator`, `io`,
`codecs`, `os`, `abstract/object`, `profiler-artifact`, `ext:<dist>`, `libc`). *Native entry*
= first non-dispatch native frame called by the innermost `_TAIL_CALL_*` handler, tagged
`call:<OPCODE>` or `op:<OPCODE>`; a GC run anywhere below the Python frame makes it `gc`.
*Stdlib entry* = outermost frame of the innermost run of stdlib frames. Non-main threads are
excluded and reported in `perf.other_thread_samples_share`. Extensions to the shared schema:
`timing`, `perf`, `reach`, `native_entry`, `stdlib_entry`, `top_python_inclusive`,
`interpreter_leaf_opcodes`, `no_python_frame_phase`, `startup_attribution`, `importtime`,
`cprofile`, `arg_size_targets`.

**Caveats.** `-X perf` (PEP 523 hook) disables inlined Python→Python calls and call
specialization (×1.00–1.23 slower; `_PyFunction_Vectorcall` entries are partly this);
trampoline frames are 1.3–3 % (`profiler-artifact`). `cpu-clock:u` (paranoid = 2) excludes
kernel time. libc has no frame pointers, so time inside libc (8 % in sphinx: under `_sre`
calls and allocation, *inferred*) is not attributed further. Arg sizes: C callables invoked
from C (`map(str.strip, …)`, `sorted(key=…)`) emit no CALL event; subjects of unbound
`Pattern`/`str.join` calls are recovered from simple load expressions only (otherwise counted
as "arg unresolved", ≤ 1.3 % of calls). Workloads are steady-state in-process loops; pytest
and coverage re-import their test modules each run, pylint clears astroid's cache.

## 7. Per-workload notes (perf shares of main-thread samples)

- **pyflakes** — `ast.parse` 21.2 % (`compile` 62 calls, files p50 51 k chars; parser 9.4 %,
  AST allocation), GC 16.7 %, pyflakes bytecode 33 %; `isinstance` 2.95 M calls/run.
- **pycodestyle** — pycodestyle bytecode 51.8 %, opcode-internal 26.9 % (incl. `finditer`
  iteration 10.2 %), re-engine 13.2 % (`match`/`search`/`finditer` on logical lines p50
  22–27 chars), stdlib 7.1 % (`tokenize` generator + `TokenInfo._make`); `getattr` 1.77 M
  calls/run (check dispatch). No GC.
- **pylint** — GC 30.4 % (astroid node graphs), astroid bytecode 27 %, `ast.parse` 5.4 %,
  `namedtuple._make` 7.3 % self (tokens; mostly GC it triggers), `cached_property.__get__`
  4.3 %.
- **black** — bytecode 59 % (blib2to3 parser `_addtoken` 9.7 %, pytokens 15.9 % owner),
  GC 9.0 %; `len` 2.2 M and `str.lower` 0.72 M calls/run (3-char strings).
- **isort** — §5.
- **pytest** — GC 33.8 % (includes 6 explicit `gc.collect()` per run, 318 ms in cProfile:
  4 from networkx's `test_memory_leak`, 2 from pytest's `unraisableexception.gc_collect_harder`),
  pytest 38.6 % / networkx 40.5 % owners, `inspect.signature` 2.0 %, warnings filters 1.4 %.
- **sphinx** — pygments 35.7 % owner, `Pattern.match` entry 20.6 % (2.49 M calls),
  docutils 27 %, Jinja2 6.9 %, `logging` 3.1 %, `inspect.getsource` 1.4 %, GC 6.9 %.
- **pip** — stdlib 37.6 % (innermost owners `email` 11.9 %, `zipfile` 8.7 %, `pathlib`
  4.5 %, `importlib` 3.3 %; entries `ZipFile.read` 5.6 %, `Distribution.metadata` 4.7 %,
  `ZipFile.__init__` 3.0 %, `PurePath.__hash__` 2.9 %, `Parser.parsestr` 2.5 %); pip bytecode
  28 %; `str.lower` 285 k, `str.split` 88 k calls/run; kernel 4.5 %.
- **coverage** — sysmon core: coverage's Python callbacks 13.0 % owner; pyflakes under test
  59 %; `compile`/`ast.parse` 10.9 % (reporter and tests), GC 12.5 %.
- **sqlglot** — bytecode 52 %, `CALL_ISINSTANCE` 8.6 % inlined-builtin share (3.44 M calls),
  attr/type 15 %, `copy.deepcopy` 5.1 %, GC 7.1 %.
- **rich** — bytecode 50 %, `Pattern.match` (pygments) 6.1 %, `lru_cache` wrapper 3.9 %,
  namedtuple `__new__` 3.5 %, `cell_len` 4.9 % self.

## 8. Decisions / open questions for the lead

1. **GC is the largest native cost** (10.9 % mean, ≤ 34 %) and outside every entry point.
   Should Rustique consider an opt-in, semantics-preserving GC lever (e.g. `gc.freeze()` after
   imports, threshold tuning)? It is a new behaviour, not an accelerated entry point —
   needs a decision against "no new APIs".
2. **Specialized opcodes hide the hottest builtins**: `isinstance`, `len`, `list.append`,
   `set()/reversed()` construction never reach `ml_meth`. Accept as out of scope (my
   recommendation: yes — they are already inline C), so devtools gains must come from
   stdlib functions.
3. **First devtools targets** (my recommendation, by share × breadth): `copy.deepcopy`
   (+ `dataclasses.asdict`), then `re` per-call overhead for `Pattern.match`/`finditer`, then
   `tokenize` token construction and pip's `zipfile`/`email` metadata path. `ast.parse` is
   large but overlaps upstream's parser plans and needs an exact AST.
4. **Startup**: `marshal.loads` (13.7 %) is the one sizeable C entry; finalization (16 %) and
   kernel time (12–15 %) are not reachable. Worth a separate measurement of
   `rustique.enable()`'s own import cost against these ~94 ms/process (never-slower rule).

Files: workloads, `README.md`, `requirements.txt`, `data/pip_index.json`,
`profile_devtools.py` in `benches/workloads/devtools/`; profiles in
`~/rustique-research/profiles/devtools/` (12 workloads + `isort_stdlib.pure-python.json`);
scratch and venvs in `~/rustique-research/w-devtools/`.
