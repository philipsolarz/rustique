#!/bin/bash
# Full suite + CPython conformance for the copy/dataclasses targets on this build.
b=$1
tag=${2:-run}
python -m pytest -q -p no:cacheprovider > /tmp/final-pytest-$b.log 2>&1
python tests/conformance/run_target_suites.py --targets copy,dataclasses \
  --modules test.test_copy,test.test_copyreg,test.test_dataclasses \
  --expect-installed copy.deepcopy,copy.copy,dataclasses.asdict,dataclasses.astuple \
  --json /tmp/final-conf-$b.json > /tmp/final-conf-$b.log 2>&1
echo "$b pytest: $(grep -aE '[0-9]+ (passed|failed)' /tmp/final-pytest-$b.log | tail -1)" > /tmp/final-$b-$tag.done
echo "$b conformance: $(tail -5 /tmp/final-conf-$b.log | tr '\n' ' ')" >> /tmp/final-$b-$tag.done
