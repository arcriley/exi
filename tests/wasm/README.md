# EXI WebAssembly integration tests

The Playwright suite builds a test-only WebAssembly module with the fixed
reading grammar from the [ESP32-C6 demo](../../examples/esp32c6/README.md)
in both byte-aligned and bit-packed builds.
Each test loads it through `WebAssembly.instantiateStreaming` and passes
EXI bytes between JavaScript and Wasm linear memory. Playwright supplies the
page and the Wasm response, so the suite needs no web server.

With pytest-playwright, the target, and Chromium installed, run from the
repository root:

```sh
rustup target add --toolchain nightly wasm32-unknown-unknown
python3 -m pytest -q tests/wasm
```

To test all three Playwright engines, install their browsers with
`playwright install chromium firefox webkit`, then run:

```sh
python3 -m pytest -q tests/wasm --browser chromium --browser firefox --browser webkit
```
