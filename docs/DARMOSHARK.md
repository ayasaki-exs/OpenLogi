# Darmoshark driver registration (experimental)

This branch registers Darmoshark (MOJI) mice as **standalone raw-HID devices**
and, as of the P2 stage, drives their **DPI** and **report rate** through
OpenLogi's existing pointer UI and JSON API. It is still an experimental,
scope-expanding integration: battery, firmware, lighting, LOD, buttons,
profiles and reconnect re-apply are not implemented.

The target hardware is the Darmoshark M3S Pro (`1915:073c` wired) and its
1K NRF Receiver (`1915:0723`). Both publish the vendor configuration channel
on usage page `0xff0a`, usage `0x0001`, as a 64-byte non-numbered
input/output report pair.

## What is registered

| USB identity | Presented as | Driver id |
|---|---|---|
| `1915:073c` | Darmoshark M3S Pro | `darmoshark` |
| `1915:0723` | Darmoshark M3S Pro (2.4 GHz) | `darmoshark` |

The receiver is presented as the same product over 2.4 GHz rather than as a
receiver card: its configuration collection is the one that reaches the mouse.

The device is a `DeviceKind::Mouse` with driver-advertised capabilities:

```rust
Capabilities {
    pointer: true,       // DPI read/write implemented by write/darmoshark.rs
    report_rate: true,   // report-rate read/write implemented there
    ..Capabilities::default()
}
```

These are **driver capabilities, not HID++ feature bits**. Only implemented
controls are advertised: buttons and lighting stay off. On Linux and macOS the
Pointer tab therefore shows the DPI and report-rate controls (plus the
host-side software scroll-inversion card).

## Code map

| Path | Role |
|---|---|
| `crates/openlogi-device-registry/src/darmoshark.rs` | Pure identities: VID/PIDs, config usage page/usage, descriptor lookup |
| `crates/openlogi-device/src/darmoshark.rs` | Protocol codec: 64-byte frame, checksum, command builders, `dms` response parsers |
| `crates/openlogi-device/src/backend.rs` | `RawChannel`: raw request/response HID channel |
| `crates/openlogi-hid/src/transport/native.rs` | `NativeRawChannel` over `async-hid` reader + writer |
| `crates/openlogi-device/src/write/darmoshark.rs` | DPI / report-rate driver: handshake, retry, read-modify-write, save, read-back |
| `crates/openlogi-agent-core/src/hardware/context.rs` | Raw driver entry points |
| `crates/openlogi-agent-core/src/orchestrator.rs` | `SharedHandles` raw-route branches for DPI / report-rate reads and writes |
| `crates/openlogi-agent/src/server.rs` | IPC `read_dpi` / `read_report_rate` use the shared raw-aware methods |
| `crates/openlogi-device/src/inventory/standalone.rs` | Recognizes the config collection and advertises the two capabilities |

No IPC method, TOML schema or `PROTOCOL_VERSION` change is involved. The raw
channel is an additive `HidBackend` capability; replay/scripted backends keep
the default "unsupported" implementation.

## What works on hardware

Verified on 2026-11-05 against `1915:0723` (firmware `0.1.1`, `dms` contract):

- `read_dpi`: current **1000**, supported **50..30000 step 50**.
- `set_dpi(800)`: read-back **800**; restored to **1000**.
- `read_report_rate`: current **500 Hz**, supported **125 / 500 / 1000 / 2000**.
- `set_report_rate(1000)`: read-back **1000 Hz**; restored to **500**.
- The device's five DPI levels are `dms` **X/Y pairs** at bytes 8..28; the
  active index is the zero-based `resp[7]`. A report-rate write is the short
  `[4, 0, 132, 4, level+1]` command, not the full sensor block.

The agent stores nothing for this device because its raw route has no serial
and therefore a transient `id:…` identity. A successful GUI/API write is
applied live, but `config.toml` persistence and reconnect re-apply are P3.

## Current limits

1. **No persistence / reconnect re-apply** — transient identity, by design of
   `PhysicalDeviceKey`.
2. **No battery or firmware** — `StandaloneDevice` has no such fields; adding
   them crosses the append-only GUI↔agent bincode format and needs a
   `PROTOCOL_VERSION` bump.
3. **No lighting** — the device reports `mode=0`, which the current
   static/breathing/color-cycle mapping does not cover.
4. **No LOD, angle, buttons, profiles or macros** — the protocol is known,
   but OpenLogi has no matching panel/IPC; these remain P3/P4.
5. **Five DPI levels only** — six-to-ten-level models use the extended `0xBC`
   sensor command, which this driver does not implement.
6. **No recording/replay raw channel** — the native backend rejects raw
   request/response recording for now.

## Verification

```sh
cargo fmt --all -- --check
cargo test -p openlogi-device darmoshark
cargo test -p openlogi-device standalone
cargo test -p openlogi-device-registry darmoshark
cargo clippy -p openlogi-device-registry -p openlogi-device --all-targets -- -D warnings
```

A minimal real-device probe calling the same functions printed:

```text
read_dpi: current=1000 supported=50..30000 step=50
read_rate: current=500 supported=[125, 500, 1000, 2000]
set_dpi(800): readback=800
restore_dpi(1000): readback=1000
set_rate(1000): readback=1000
restore_rate(500): readback=500
```

`openlogi list` still consumes the standalone inventory; the device appears as
`Darmoshark M3S Pro (2.4 GHz)`. Full GUI/agent packaging was not built here.

## Scope and risk

OpenLogi is a Logitech HID++ companion. Registering a non-Logitech vendor is a
product-scope expansion, not a neutral driver addition. This branch is a proof
of concept for the standalone-driver seam; it is not a claim that upstream will
accept the device family. The codec, raw channel and driver are isolated so the
experiment can be dropped without touching the HID++ feature layer.
