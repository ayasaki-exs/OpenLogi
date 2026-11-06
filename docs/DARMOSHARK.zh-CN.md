# Darmoshark 驱动注册（实验性）

本分支把 Darmoshark（MOJI）鼠标注册为 **standalone raw-HID 设备**，并在 P2
阶段把 **DPI** 和 **回报率** 接到 OpenLogi 现有的指针 UI 与 JSON API。它仍是
实验性、扩大产品范围的接入：电量、固件、灯效、LOD、按键、profile 和重连 reapply
都尚未实现。

目标硬件是 Darmoshark M3S Pro（`1915:073c` 有线）和它的 1K NRF Receiver
（`1915:0723`）。两者都在 usage page `0xff0a`、usage `0x0001` 上暴露厂商配置
通道，使用 64 字节、无 report id 的输入/输出报文。

## 注册了什么

| USB 身份 | 显示为 | driver id |
|---|---|---|
| `1915:073c` | Darmoshark M3S Pro | `darmoshark` |
| `1915:0723` | Darmoshark M3S Pro (2.4 GHz) | `darmoshark` |

接收器被表示为同一产品的 2.4 GHz 形态，而不是一张独立的接收器卡片：它的配置
集合才是真正能到达鼠标的那一个。

设备在 standalone inventory 中是一个 `DeviceKind::Mouse`，并声明：

```rust
Capabilities {
    pointer: true,       // DPI 读写由 write/darmoshark.rs 实现
    report_rate: true,   // 回报率读写同样在那里实现
    ..Capabilities::default()
}
```

这是**驱动能力声明，不是 HID++ feature bit**。只声明已经实现的控件：buttons
和 lighting 仍为 false。Linux/macOS 上 Pointer 标签页因此会出现 DPI 与回报率
控件（以及主机侧软件滚轮反转卡片）。

## 代码地图

| 路径 | 作用 |
|---|---|
| `crates/openlogi-device-registry/src/darmoshark.rs` | 纯身份：VID/PID、config usage、描述符查找 |
| `crates/openlogi-device/src/darmoshark.rs` | 协议 codec：64 字节帧、checksum、命令构造、`dms` 响应解析 |
| `crates/openlogi-device/src/backend.rs` | `RawChannel`：raw 请求/响应 HID 通道 |
| `crates/openlogi-hid/src/transport/native.rs` | 基于 `async-hid` reader + writer 的 `NativeRawChannel` |
| `crates/openlogi-device/src/write/darmoshark.rs` | DPI / 回报率驱动：握手、重试、读改写、save、回读 |
| `crates/openlogi-agent-core/src/hardware/context.rs` | raw 驱动入口 |
| `crates/openlogi-agent-core/src/orchestrator.rs` | `SharedHandles` 中 DPI / 回报率的 raw-route 分支 |
| `crates/openlogi-agent/src/server.rs` | IPC `read_dpi` / `read_report_rate` 走 shared 方法 |
| `crates/openlogi-device/src/inventory/standalone.rs` | 识别 config 集合并声明两项能力 |

不涉及 IPC 方法、TOML schema 或 `PROTOCOL_VERSION` 变更。raw channel 是
`HidBackend` 的增量能力；replay/scripted backend 保留默认的“不支持”实现。

## 真机上已经可用

2026-11-05 对 `1915:0723`（固件 `0.1.1`、`dms` contract）验证：

- `read_dpi`：current **1000**，supported **50..30000 step 50**。
- `set_dpi(800)`：回读 **800**；恢复为 **1000**。
- `read_report_rate`：current **500 Hz**，supported **125 / 500 / 1000 / 2000**。
- `set_report_rate(1000)`：回读 **1000 Hz**；恢复为 **500**。
- 设备的五档 DPI 是字节 8..28 的 `dms` **X/Y 对**；当前档位是零基
  `resp[7]`。回报率写入是短命令 `[4, 0, 132, 4, level+1]`，不是完整 sensor
  block。

该设备没有序列号，raw route 身份是瞬态 `id:…`，所以 agent 不保存配置：
GUI/API 写入会立即生效，但 `config.toml` 持久化和重连 reapply 属于 P3。

## 当前限制

1. **无持久化 / 重连 reapply** —— 瞬态身份，`PhysicalDeviceKey` 的既有设计。
2. **无电量/固件** —— `StandaloneDevice` 没有这些字段；新增字段会穿过
   append-only 的 GUI↔agent bincode 格式，需要 bump `PROTOCOL_VERSION`。
3. **无灯效** —— 设备返回 `mode=0`，现有 static/breathing/color-cycle 映射
   不覆盖它。
4. **无 LOD、angle、按键、profile、宏** —— 协议已知，但 OpenLogi 没有对应
   面板/IPC，属于 P3/P4。
5. **仅支持五档 DPI** —— 6–10 档机型使用扩展 `0xBC` sensor 命令，本驱动
   未实现。
6. **无 raw channel 录制/replay** —— native backend 暂时拒绝 raw 请求/响应
   录制。

## 验证

```sh
cargo fmt --all -- --check
cargo test -p openlogi-device darmoshark
cargo test -p openlogi-device standalone
cargo test -p openlogi-device-registry darmoshark
cargo clippy -p openlogi-device-registry -p openlogi-device --all-targets -- -D warnings
```

一个调用相同驱动函数的最小真机 probe 输出：

```text
read_dpi: current=1000 supported=50..30000 step=50
read_rate: current=500 supported=[125, 500, 1000, 2000]
set_dpi(800): readback=800
restore_dpi(1000): readback=1000
set_rate(1000): readback=1000
restore_rate(500): readback=500
```

`openlogi list` 仍使用同一份 standalone inventory，设备会显示为
`Darmoshark M3S Pro (2.4 GHz)`。本次没有构建完整 GUI/agent 安装包。

## 范围与风险

OpenLogi 是 Logitech HID++ 的伴侣应用。注册非 Logitech 厂商属于产品范围扩展，
不是中性的驱动新增。本分支是 standalone 驱动接缝的 PoC，不代表上游会接受该
设备族。codec、raw channel 和驱动彼此隔离，因此这个实验可以整体丢弃而不触碰
HID++ feature 层。
