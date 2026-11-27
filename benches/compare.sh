#!/usr/bin/env bash
# Compares the `exi` tool with other XML and EXI tools: output sizes, and times with
# hyperfine. Tools that are not installed are skipped.
#
#   benches/compare.sh                  # the generated documents and benches/data/*
#   benches/compare.sh logo.svg ...     # these documents
#   benches/compare.sh page.html ...    # HTML pages: HTML vs XHTML5 EXI
#
# Uses: hyperfine, xmllint, gzip, xz, zstd, and EXIficient (java with the jars from
# fixtures/fetch-exificient.sh, or EXIFICIENT_CP set to a classpath).
#
# For each document it learns a schema from the document itself (`exi infer`), so the
# strict rows show the best case: a schema that fits exactly.
#
# HTML pages (.html, .htm) get their own rows: the page, the page compressed, and EXI
# of the parsed page, schema-less and with the XHTML5 profile. These need the html5ever
# and xhtml5 features, which the build line below turns on.
#
# The times are of whole processes: start-up, reading and writing files, and for
# EXIficient the JVM's start-up. A pipeline (`exi encode | zstd`) is two processes. For
# library times in one process, use `cargo bench`. Output goes to target/compare: a
# markdown and a JSON table for each document, and meta.txt with the tool versions and
# machine.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
work=$root/target/compare

# Check named documents before building: paths are taken from where the script was run.
docs=()
for f in "$@"; do
    if [ ! -f "$f" ]; then
        echo "compare.sh: no such file: $f" >&2
        exit 1
    fi
    docs+=("$(cd "$(dirname "$f")" && pwd)/$(basename "$f")")
done

mkdir -p "$work"
cd "$root"

cargo build --quiet --release --features infer,xhtml5,html5ever
exi=$root/target/release/exi

if [ ${#docs[@]} -eq 0 ]; then
    cargo bench --quiet --features infer --bench throughput -- --corpus "$work/corpus" >/dev/null
    docs=("$work"/corpus/*)
    for f in benches/data/*.xml benches/data/*.svg; do
        [ -e "$f" ] && docs+=("$f")
    done
fi

have() { command -v "$1" >/dev/null 2>&1; }
# A classpath of jars joined with `:` (a `*` would be expanded by the shell).
cp=${EXIFICIENT_CP:-$(ls "$root"/fixtures/jars/*.jar 2>/dev/null | paste -sd: -)}
exificient=""
if have java && [ -n "$cp" ]; then
    exificient="java -cp $cp com.siemens.ct.exi.main.cmd.EXIficientCMD"
fi
version() { # version TOOL ARGS...: its first line of version output, or nothing
    have "$1" && "$@" 2>&1 | head -n 1 || true
}
{
    echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo "git_commit: $(git rev-parse HEAD 2>/dev/null)"
    echo "git_dirty: $([ -n "$(git status --porcelain --untracked-files=no 2>/dev/null)" ] && echo true || echo false)"
    echo "rustc: $(rustc -V)"
    echo "exi: $("$exi" --version 2>&1 | head -n 1)"
    echo "hyperfine: $(version hyperfine --version)"
    echo "xmllint: $(version xmllint --version)"
    echo "gzip: $(version gzip --version)"
    echo "xz: $(version xz --version)"
    echo "zstd: $(version zstd --version)"
    echo "brotli: $(version brotli --version)"
    echo "java: $(version java -version)"
    echo "exificient_classpath: $cp"
    echo "cpu: $(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ //')"
    echo "kernel: $(uname -sr)"
    echo "governor: $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>/dev/null)"
    echo "boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)"
} >"$work/meta.txt"
size() { wc -c <"$1" | tr -d ' '; }
row() { printf '%-40s %10s\n' "$1" "$(size "$2")"; }
squeeze() { # squeeze LABEL FILE: the file under each installed compressor
    if have gzip; then gzip -9c "$2" >"$2.gz"; row "$1 + gzip -9" "$2.gz"; fi
    if have xz; then xz -9c "$2" >"$2.xz"; row "$1 + xz -9" "$2.xz"; fi
    if have zstd; then zstd -q -19 -c "$2" >"$2.zst"; row "$1 + zstd -19" "$2.zst"; fi
    if have brotli; then brotli -q 11 -c "$2" >"$2.br"; row "$1 + brotli -11" "$2.br"; fi
}

html_page() {
    local doc=$1 name d
    name=$(basename "$doc")
    d=$work/$name.d
    rm -rf "$d"
    mkdir -p "$d"
    cp "$doc" "$d/in.html"
    "$exi" encode --html "$d/in.html" -o "$d/exi"
    "$exi" encode --html --xhtml5 "$d/in.html" -o "$d/x5.exi"
    "$exi" encode --html --xhtml5 -c pre "$d/in.html" -o "$d/x5pre.exi"
    "$exi" encode --html --xhtml5 -c compression "$d/in.html" -o "$d/x5comp.exi"
    "$exi" xhtml "$d/in.html" -o "$d/in.xhtml"
    echo
    echo "== $name"
    printf '%-40s %10s\n' "form" "octets"
    row "HTML" "$d/in.html"
    squeeze "HTML" "$d/in.html"
    row "XHTML (the parsed page as XML)" "$d/in.xhtml"
    row "exi --html (schema-less)" "$d/exi"
    squeeze "exi --html" "$d/exi"
    row "exi --html --xhtml5" "$d/x5.exi"
    squeeze "exi --html --xhtml5" "$d/x5.exi"
    row "exi --html --xhtml5 -c pre" "$d/x5pre.exi"
    squeeze "exi --html --xhtml5 -c pre" "$d/x5pre.exi"
    row "exi --html --xhtml5 -c compression" "$d/x5comp.exi"
    if ! have hyperfine; then
        return
    fi
    cmds=(
        -n "exi encode --html --xhtml5" "$exi encode --html --xhtml5 $d/in.html -o $d/t.exi"
        -n "exi decode --html (XHTML5)" "$exi decode --html $d/x5.exi -o $d/t.html"
        -n "exi encode --html --xhtml5 -c compression" "$exi encode --html --xhtml5 -c compression $d/in.html -o $d/t.c.exi"
        -n "exi decode --html (XHTML5, compression)" "$exi decode --html $d/x5comp.exi -o $d/t.c.html"
        -n "exi encode --html" "$exi encode --html $d/in.html -o $d/t2.exi"
        -n "exi decode --html" "$exi decode --html $d/exi -o $d/t2.html"
    )
    if have gzip; then cmds+=(-n "gzip -6 (HTML)" "gzip -6c $d/in.html > $d/t.gz"); fi
    if have brotli; then cmds+=(-n "brotli -5 (HTML)" "brotli -q 5 -c $d/in.html > $d/t.br"); fi
    hyperfine --shell=bash --style basic --warmup 2 --export-markdown "$work/$name.md" --export-json "$work/$name.json" "${cmds[@]}"
    echo "(whole-process times; tables: $work/$name.md, $name.json)"
}

for doc in "${docs[@]}"; do
    case $doc in
    *.html | *.htm)
        html_page "$doc"
        continue
        ;;
    esac
    name=$(basename "$doc")
    d=$work/$name.d
    rm -rf "$d"
    mkdir -p "$d"
    cp "$doc" "$d/in.xml"

    "$exi" infer --exact "$d/in.xml" -d "$d/xsd" -o "$d/schema.exig" 2>/dev/null
    "$exi" encode "$d/in.xml" -o "$d/exi"
    "$exi" encode -c bit "$d/in.xml" -o "$d/bit.exi"
    "$exi" encode -c pre "$d/in.xml" -o "$d/pre.exi"
    "$exi" encode -c compression "$d/in.xml" -o "$d/comp.exi"
    "$exi" encode -s "$d/schema.exig" "$d/in.xml" -o "$d/strict.exi"
    "$exi" encode -c compression -s "$d/schema.exig" "$d/in.xml" -o "$d/strict-comp.exi"

    echo
    echo "== $name"
    printf '%-40s %10s\n' "form" "octets"
    row "XML" "$d/in.xml"
    if have gzip; then gzip -9c "$d/in.xml" >"$d/xml.gz"; row "XML + gzip -9" "$d/xml.gz"; fi
    if have xz; then xz -9c "$d/in.xml" >"$d/xml.xz"; row "XML + xz -9" "$d/xml.xz"; fi
    if have zstd; then zstd -q -19 -c "$d/in.xml" >"$d/xml.zst"; row "XML + zstd -19" "$d/xml.zst"; fi
    row "exi (schema-less)" "$d/exi"
    if have gzip; then gzip -9c "$d/exi" >"$d/exi.gz"; row "exi + gzip -9" "$d/exi.gz"; fi
    if have xz; then xz -9c "$d/exi" >"$d/exi.xz"; row "exi + xz -9" "$d/exi.xz"; fi
    if have zstd; then zstd -q -19 -c "$d/exi" >"$d/exi.zst"; row "exi + zstd -19" "$d/exi.zst"; fi
    row "exi -c bit (bit-packed)" "$d/bit.exi"
    row "exi -c pre (values by name)" "$d/pre.exi"
    if have xz; then xz -9c "$d/pre.exi" >"$d/pre.xz"; row "exi -c pre + xz -9" "$d/pre.xz"; fi
    if have zstd; then zstd -q -19 -c "$d/pre.exi" >"$d/pre.zst"; row "exi -c pre + zstd -19" "$d/pre.zst"; fi
    row "exi -c compression (DEFLATE)" "$d/comp.exi"
    row "exi strict (inferred schema)" "$d/strict.exi"
    if have xz; then xz -9c "$d/strict.exi" >"$d/strict.xz"; row "exi strict + xz -9" "$d/strict.xz"; fi
    if have zstd; then zstd -q -19 -c "$d/strict.exi" >"$d/strict.zst"; row "exi strict + zstd -19" "$d/strict.zst"; fi
    row "exi strict -c compression" "$d/strict-comp.exi"
    row "  the schema (.exig)" "$d/schema.exig"
    if have xmllint; then
        # EXIficient's command line drops whitespace-only text (indentation). These rows
        # give exi the same document, for sizes that compare like with like.
        xmllint --noblanks "$d/in.xml" >"$d/noblanks.xml"
        "$exi" encode --bare "$d/noblanks.xml" -o "$d/nb.exi"
        row "exi, no indentation, bare header" "$d/nb.exi"
        if have xz; then xz -9c "$d/nb.exi" >"$d/nb.xz"; row "  + xz -9" "$d/nb.xz"; fi
        if have zstd; then zstd -q -19 -c "$d/nb.exi" >"$d/nb.zst"; row "  + zstd -19" "$d/nb.zst"; fi
    fi
    if [ -n "$exificient" ]; then
        # These drop the indentation: compare with the "no indentation" rows.
        $exificient -encode -i "$d/in.xml" -o "$d/exif.bit" >/dev/null 2>&1 && row "EXIficient bit-packed" "$d/exif.bit"
        $exificient -encode -i "$d/in.xml" -o "$d/exif.byte" -bytePacked >/dev/null 2>&1 && row "EXIficient byte-aligned" "$d/exif.byte"
        $exificient -encode -i "$d/in.xml" -o "$d/exif.deflate" -compression >/dev/null 2>&1 && row "EXIficient compression" "$d/exif.deflate"
        $exificient -encode -i "$d/in.xml" -o "$d/exif.strict" -schema "$d/xsd/schema.xsd" -strict >/dev/null 2>&1 && row "EXIficient strict bit-packed" "$d/exif.strict"
        $exificient -encode -i "$d/in.xml" -o "$d/exif.strictc" -schema "$d/xsd/schema.xsd" -strict -compression >/dev/null 2>&1 && row "EXIficient strict compression" "$d/exif.strictc"
    fi

    if ! have hyperfine; then
        echo "(hyperfine is not installed: no times)"
        continue
    fi
    # Times. Every command reads a file and writes a file, so all pay the same I/O.
    cmds=(
        -n "exi encode" "$exi encode $d/in.xml -o $d/t.exi"
        -n "exi encode strict" "$exi encode -s $d/schema.exig $d/in.xml -o $d/t.sexi"
        -n "exi decode" "$exi decode $d/exi -o $d/t.xml"
        -n "exi decode strict" "$exi decode -s $d/schema.exig $d/strict.exi -o $d/t.sxml"
        -n "exi encode -c bit" "$exi encode -c bit $d/in.xml -o $d/t.bit"
        -n "exi decode -c bit" "$exi decode $d/bit.exi -o $d/t.bxml"
        -n "exi encode -c compression" "$exi encode -c compression $d/in.xml -o $d/t.comp"
        -n "exi decode -c compression" "$exi decode $d/comp.exi -o $d/t.cxml"
    )
    if have xmllint; then
        cmds+=(-n "xmllint parse" "xmllint --noout $d/in.xml")
        cmds+=(-n "xmllint parse+write" "xmllint --output $d/t.lint $d/in.xml")
    fi
    if have gzip; then cmds+=(-n "gzip -6" "gzip -6c $d/in.xml > $d/t.gz"); fi
    if have zstd; then
        cmds+=(-n "zstd -3 (XML)" "zstd -q -3 -f $d/in.xml -o $d/t.zst")
        cmds+=(-n "exi encode | zstd -3" "$exi encode $d/in.xml | zstd -q -3 > $d/t.exi.zst")
    fi
    if [ -n "$exificient" ]; then
        # The JVM's start-up is in these times: see EXIficient's own timing for steady state.
        cmds+=(-n "EXIficient encode" "$exificient -encode -i $d/in.xml -o $d/t.exif -bytePacked")
        cmds+=(-n "EXIficient decode" "$exificient -decode -i $d/exif.byte -o $d/t.exif.xml -bytePacked")
    fi
    hyperfine --shell=bash --style basic --warmup 2 --export-markdown "$work/$name.md" --export-json "$work/$name.json" "${cmds[@]}"
    echo "(whole-process times; tables: $work/$name.md, $name.json)"
done
