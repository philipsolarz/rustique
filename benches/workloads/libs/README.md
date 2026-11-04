# Workloads: `libs` family (third-party-heavy code, wave 2)

Programs whose time is dominated by popular third-party libraries, chosen by reach (all six
are top-100 PyPI downloads; boto3/botocore are #1/#2) and by diversity of what the library
does (SDK client machinery, Rust-core validation, pure-Python validation, parsing,
generated code, tree building). Module format and rules:
`~/rustique-research/w2/PHASE1_BRIEF.md`. Install with `uv pip install -r requirements.txt`
into CPython 3.15.1 or 3.15.1t (all six run on both with identical digests). Profiles and
analysis: [`docs/atlas/w2-wl-frameworks.md`](../../../docs/atlas/w2-wl-frameworks.md).

- `boto3_client` — boto3/botocore S3 + DynamoDB (resource layer) + SQS calls through the full client path (validation, events, endpoint rules, serialization, SigV4, parsing) with canned `before-send` responses (1,140 calls).
- `pydantic_payloads` — pydantic v2 discriminated-union webhook events: `validate_json`/`validate_python`, Python validators, aliases, `model_copy`, `dump_json`, error reports (38,400 events).
- `jsonschema_validate` — jsonschema Draft 2020-12 with `$ref`, `oneOf`, `if/then`, formats; `is_valid`/`validate` and `iter_errors` + `best_match` (2,500 documents).
- `yaml_config` — PyYAML pure `SafeLoader`/`SafeDumper` on Kubernetes manifests, compose files with anchors/merge keys and app configs (0.44 MB; PyYAML installs from sdist on 3.15, pure Python).
- `attrs_cattrs` — attrs value objects/aggregates (exec-generated methods) structured/unstructured by cattrs-generated functions, tagged unions, `evolve` (2,000 documents).
- `bs4_scrape` — BeautifulSoup on the stdlib `html.parser` + soupsieve CSS selectors: parse, query, edit and re-serialize 24 product pages (1.8 MB).

Left out (covered elsewhere or similar shape): markdown/mistune + pygments (regex-heavy
pure Python, like `sphinx_autodoc`/`rich_render` in `devtools`), httpx (its client and ASGI
transport already run in `web/fastapi_pydantic` and `frameworks/fastapi_sqlalchemy`),
marshmallow (in `frameworks/flask_marshmallow`).
