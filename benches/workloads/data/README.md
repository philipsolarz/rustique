# Workloads: `data` family (data processing & scripting)

Each module has `REQUIRES`, `setup()`, `run(state)`, `digest(result)` (format: Phase-1 brief).
Inputs are generated deterministically in `setup()`; digests are stable across runs and
`PYTHONHASHSEED` values, and identical on 3.15.1t. Run time is the median `run()` on CPython
3.15.1 (PBS build) on the shared WSL2 box (*measured*, `_profiler.py profile`, ±~20% noise).

| workload | stands for | run() |
|---|---|---|
| `csv_etl` | nightly CSV export → clean/convert (`csv.DictReader`, `fromisoformat`, `strftime`) → JSON lines + summary CSV | 0.63 s |
| `log_analytics` | access-log + syslog analysis: `re` named groups, `strptime`, `urllib.parse`, `ipaddress`, `Counter` | 1.36 s |
| `tz_events` | calendar/scheduling job: `fromisoformat`/`strptime`, `zoneinfo` conversions, `timedelta`, `strftime` | 0.74 s |
| `text_corpus` | text normalisation + word stats: `unicodedata`, `casefold`, `re.findall`, `translate`, `Counter`, TF-IDF | 0.80 s |
| `fs_inventory` | file-tree scan: `os.walk` + `fnmatch`, `scandir`, `pathlib.rglob`, `glob` on a generated 13k-file tree | 0.40 s (40% sys) |
| `backup_archive` | backup/packaging: `hashlib`, `zipfile`, `tarfile` (gz), `gzip`, `zlib.crc32`, `base64` | 0.68 s |
| `config_merge` | deploy tooling config load: `configparser`, `tomllib`, PyYAML `safe_load`/`safe_dump`, `json`, deep merge | 0.74 s |
| `xml_feeds` | product feed / sitemap processing: `ElementTree` parse/find/build/tostring, `iterparse`, `minidom` | 0.58 s |
| `state_snapshot` | object-graph persistence: `copy.deepcopy`, `pickle`, `dataclasses.asdict`/`replace`, `__eq__` | 0.78 s |
| `billing_engine` | invoicing: `Decimal` money math + `quantize`, `Fraction` proration, `enum`, slotted dataclasses, `statistics` | 1.08 s |
| `report_render` | fixtures + Markdown/text report: `random`, `uuid`, `secrets`, f-strings, `format`, `%`, `textwrap` | 0.53 s |
| `pandas_etl` | pandas ETL: `read_csv`, `to_datetime`, `.str`, `merge`, `groupby`, `pivot_table`, `apply` | 0.62 s |

Dependencies: `requirements.txt` (PyYAML, tzdata, pandas, numpy, python-dateutil). All
install from wheels/sdist on 3.15.1 and 3.15.1t (PyYAML builds without libyaml, i.e. the
pure-Python loader that `yaml.safe_load` uses by default; pandas keeps the GIL disabled
on 3.15.1t). Venv used for the profiles: `uv venv --python 3.15 ~/rustique-research/w-data/venv`.

`_profiler.py` (not a workload) produces the shared profile JSON for one workload:

```sh
PY=~/rustique-research/w-data/venv/bin/python
$PY _profiler.py profile csv_etl.py --python $PY --out ~/rustique-research/profiles/data/csv_etl.json
$PY _profiler.py rollup ~/rustique-research/profiles/data/*.json --out rollup.json
```

It needs `perf` (cpu-clock is enough), `nm`/`readelf`, and the CPython source tree
(`--cpython-src`, default `~/src/cpython-3.15.1`) to map symbols to components. Results:
`docs/atlas/workloads-data.md`.
