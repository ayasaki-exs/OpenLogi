# Darmoshark driver registration (experimental)

This branch registers Darmoshark (MOJI) mice as **standalone raw-HID devices**
so OpenLogi's inventory, GUI and CLI can see them. It is an experimental,
identity-only integration: the protocol codec exists, but no configuration
write path is wired to the agent yet.

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

The device appears as a `DeviceKind::Mouse` in the standalone inventory. Its
`capabilities` are an explicitly measured **empty** set, not `None`:

- `None` would make clients fall back to `Capabilities::presumed_from_kind`,
  which presumes HID++ button and pointer controls this device does not serve.
- `Some(Capabilities::default())` says "measured: no configurable HID++
  features". This follows the G600 precedent. On Linux and macOS the Pointer
  tab may still appear because software scroll inversion is a host feature;
  the DPI and report-rate capability flags are false, so neither control is
  claimed. The DPI panel may show an unsupported/read-failure state until the
  P2 write path exists.

## Code map

| Path | Role |
|---|---|
| `crates/openlogi-device-registry/src/darmoshark.rs` | Pure identities: VID/PIDs, config usage page/usage, descriptor lookup, matching tests |
| `crates/openlogi-device/src/darmoshark.rs` | Protocol codec: 64-byte frame, checksum, command builders, response parsers, encodings; no host I/O |
| `crates/openlogi-device/src/inventory/standalone.rs` | Recognizes the config collection and builds the `StandaloneDevice` |

No `Cargo.toml`, IPC, agent, desktop, core or backend changes are needed for
this stage: `openlogi-device` already depends on the registry, and the HID++
candidate filter is scoped to Logitech's vendor id, so the Darmoshark config
node never enters the HID++ path.

## Why this is identity-only

Making DPI, report rate or lighting actually work needs more than a registry
entry:

1. **No raw read path.** `HidBackend` exposes `open_raw_writer` only. The
   Darmoshark protocol is request/response, so the driver needs a raw channel
   that reads 64-byte input reports (native, replay and scripted backends).
2. **Existing IPC assumes HID++.** `read_dpi`/`set_dpi`,
   `read_report_rate`/`set_report_rate` and the lighting methods resolve a
   route to a HID++ channel. A raw device would have to be dispatched to a
   Darmoshark driver on the same route.
3. **Capability semantics.** `Capabilities` is a HID++ feature-table DTO used
   for UI gating. A raw driver needs either a documented extension of that
   meaning or a separate driver-capability descriptor.
4. **Battery and firmware.** `StandaloneDevice` has no battery or model-info
   fields. Adding them crosses the append-only GUI↔agent bincode wire format
   and requires a `PROTOCOL_VERSION` bump.

Until those land, claiming `pointer`/`report_rate`/`lighting` capabilities
would render panels that cannot read or write the device — worse than an
identity-only card.

## Mapping summary

The full inventory and one-to-one mapping is maintained alongside the
Darmoshark driver repository (`docs/OPENLOGI-FEATURE-MAP.md`). In short:

| Darmoshark feature | OpenLogi surface | Status |
|---|---|---|
| Identity, device card, alias | standalone inventory | this branch |
| Battery / charging | `StandaloneDevice` + inventory read | phase 3 |
| Firmware / device info | `StandaloneDevice` + model info | phase 3 |
| Current DPI read/write | existing DPI IPC | phase 2 |
| Report rate | existing report-rate IPC | phase 2 |
| LOD / angle | no OpenLogi panel | phase 3 |
| Static / breathing / color-cycle lighting | keyboard RGB panel is HID++-specific | phase 3 |
| Button remapping | HID++ ReprogControls / gaming tables | partial; raw action table unknown |
| Onboard profile switch | no device-profile IPC | phase 4 |
| SmartShift, thumbwheel, haptics, Easy-Switch | — | not applicable |

## Verification

```sh
cargo fmt --all -- --check
cargo test -p openlogi-device-registry darmoshark
cargo test -p openlogi-device darmoshark
cargo test -p openlogi-device standalone
cargo clippy -p openlogi-device-registry -p openlogi-device --all-targets -- -D warnings
cargo run -p openlogi -- list
```

With the wireless receiver attached, `openlogi list` should include a
Darmoshark mouse record named `Darmoshark M3S Pro (2.4 GHz)`. Verified on
2026-11-05 with a minimal harness calling
`openlogi_hid::enumerate_standalone()`: it returned exactly one record with
`kind=Mouse`, `driver=darmoshark`, `vid=1915 pid=0723 usage=ff0a:0001`, online,
and the measured empty capability set. No write path was exercised, and no
device permission is required for enumeration.

## Scope and risk

OpenLogi is a Logitech HID++ companion. Registering a non-Logitech vendor is a
product-scope expansion, not a neutral driver addition. This branch is a
proof of concept meant to answer whether the standalone-driver seam is usable
for the Darmoshark protocol; it is not a claim that upstream will accept the
device family. The codec and identity modules are isolated so the experiment
can be dropped without touching the HID++ layer.
