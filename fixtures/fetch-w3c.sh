#!/bin/sh
# Downloads the W3C EXI test suite's interoperability data, its test case configuration
# and EXIficient's 2013 encodings of it into fixtures/w3c, for tests/w3c.rs. Needs git
# and unzip. Run from the repository root.
set -e
C=23d5f587ba3ec281f1a6a8182c1450cb870b469e
D=fixtures/w3c
rm -rf "$D"
mkdir -p "$D"
git -C "$D" init -q
git -C "$D" remote add origin https://github.com/w3c/exi-testsuite
git -C "$D" sparse-checkout set --no-cone /data/interop/ /config/testCases-interop/ \
    /encodings/EXIficient/2013-12-05_EXIficient.zip
git -C "$D" fetch -q --depth 1 --filter=blob:none origin "$C"
git -C "$D" checkout -q FETCH_HEAD
unzip -q "$D/encodings/EXIficient/2013-12-05_EXIficient.zip" -d "$D/encodings/EXIficient"
du -sh "$D"
