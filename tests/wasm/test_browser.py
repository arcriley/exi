"""Exercise the EXI codec through native WebAssembly in a browser."""

import subprocess
from pathlib import Path

import pytest


@pytest.fixture(scope="session")
def wasm_bytes():
    harness = Path(__file__).resolve().parent
    subprocess.run(
        [
            "cargo", "+nightly", "build", "--locked", "--release",
            "--target", "wasm32-unknown-unknown",
        ],
        cwd=harness,
        check=True,
    )
    return (
        harness / "target/wasm32-unknown-unknown/release/exi_wasm_test.wasm"
    ).read_bytes()


@pytest.fixture
def codec(page, wasm_bytes):
    page.route(
        "http://exi.test/",
        lambda route: route.fulfill(content_type="text/html", body="<title>EXI</title>"),
    )
    page.route(
        "http://exi.test/exi.wasm",
        lambda route: route.fulfill(content_type="application/wasm", body=wasm_bytes),
    )
    page.goto("http://exi.test/")
    page.evaluate(
        """async () => {
            const { instance } = await WebAssembly.instantiateStreaming(fetch("/exi.wasm"));
            const wasm = instance.exports;
            const memory = () =>
                new Uint8Array(wasm.memory.buffer, wasm.buffer(), wasm.capacity());
            globalThis.exiTest = {
                encode(value) {
                    const length = wasm.encode_reading(BigInt(value));
                    if (length < 0) throw new Error("encode failed");
                    return Array.from(memory().subarray(0, length));
                },
                decode(bytes) {
                    if (bytes.length <= wasm.capacity()) memory().set(bytes);
                    const status = wasm.decode_reading(bytes.length);
                    return status === 0 ? wasm.reading().toString() : null;
                },
                capacity: wasm.capacity(),
            };
        }"""
    )
    return page


def test_xiao_wire_vector(codec):
    assert codec.evaluate("exiTest.decode([0x80, 0, 0, 0x15])") == "21"
    assert codec.evaluate("exiTest.encode('22')") == [0x80, 0, 0, 0x16]


def test_signed_integer_round_trip(codec):
    readings = ["-9223372036854775808", "-21", "0", "21", "9223372036854775807"]
    assert codec.evaluate(
        "values => values.map(value => exiTest.decode(exiTest.encode(value)))",
        readings,
    ) == readings


def test_rejects_bad_documents_and_keeps_working(codec):
    rejected = codec.evaluate(
        """() => [
            [],
            [0xa0, 0, 0, 0x15],
            [0x80, 0, 0],
            [0x80, 0, 0, 0x15, 0],
            Array(exiTest.capacity + 1).fill(0),
        ].map(bytes => exiTest.decode(bytes))"""
    )
    assert rejected == [None] * 5
    assert codec.evaluate("exiTest.decode(exiTest.encode('-22'))") == "-22"
