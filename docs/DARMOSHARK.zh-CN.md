# Darmoshark 驱动注册（实验性）

本分支把 Darmoshark（MOJI）鼠标注册为 **standalone raw-HID 设备**，让
OpenLogi 的 inventory、GUI 和 CLI 能够看到它。这是实验性的“仅身份”接入：
协议 codec 已就位，但还没有把配置写入路径接到 agent。

目标硬件是 Darmoshark M3S Pro（`1915:073c`，有线）和它的 1K NRF Receiver
（`1915:0723`）。两者都在 usage page `0xff0a`、usage `0x0001` 上暴露厂商配置
通道，使用 64 字节、无 report id 的输入/输出报文。

## 注册了什么

| USB 身份 | 显示为 | driver id |
|---|---|---|
| `1915:073c` | Darmoshark M3S Pro | `darmoshark` |
| `1915:0723` | Darmoshark M3S Pro (2.4 GHz) | `darmoshark` |

接收器被表示为同一产品的 2.4 GHz 形态，而不是一张独立的接收器卡片：它的配置
集合才是真正能到达鼠标的那一个。

设备在 standalone inventory 中以 `DeviceKind::Mouse` 出现。它的 `capabilities`
是**显式测量的空集合**，而不是 `None`：

- `None` 会让客户端回退到 `Capabilities::presumed_from_kind`，从而假定该设备
  拥有它并不提供的 HID++ 按键和指针控制。
- `Some(Capabilities::default())` 表示“已测量：没有可配置的 HID++ 功能”。这与
  G600 的先例一致。Linux 和 macOS 上仍可能显示 Pointer 标签页，因为软件滚轮反转
  是主机侧功能；DPI 和回报率的能力位为 false，因此不会声称提供这两项控制。在
  P2 写入路径完成前，DPI 面板可能显示“不支持/读取失败”状态。

## 代码地图

| 路径 | 作用 |
|---|---|
| `crates/openlogi-device-registry/src/darmoshark.rs` | 纯身份：VID/PID、config usage page/usage、描述符查找与匹配测试 |
| `crates/openlogi-device/src/darmoshark.rs` | 协议 codec：64 字节帧、checksum、命令构造、响应解析、编码表；不做主机 I/O |
| `crates/openlogi-device/src/inventory/standalone.rs` | 识别 config 集合并构造 `StandaloneDevice` |

这一阶段不需要改 `Cargo.toml`、IPC、agent、desktop、core 或 backend：
`openlogi-device` 已经依赖 registry，而 HID++ 候选过滤只接受 Logitech 的
vendor id，所以 Darmoshark 的 config 节点不会误入 HID++ 路径。

## 为什么只做身份

要让 DPI、回报率或灯效真正工作，需要的远不止一条 registry 记录：

1. **没有 raw 读通道。** `HidBackend` 只有 `open_raw_writer`。Darmoshark 协议是
   请求/响应式的，驱动需要能读取 64 字节输入报文的 raw channel（native、
   replay 和 scripted backend 都要实现）。
2. **现有 IPC 假定 HID++。** `read_dpi`/`set_dpi`、`read_report_rate`/
   `set_report_rate` 以及灯效方法都会把 route 解析成 HID++ channel。raw 设备
   需要在同一 route 上分发到 Darmoshark 驱动。
3. **能力语义。** `Capabilities` 是 HID++ feature table 的 DTO，被 UI 用来决定
   面板显示。raw 驱动要么明确扩展这个语义，要么新增独立的驱动能力描述符。
4. **电量与固件。** `StandaloneDevice` 没有 battery/model-info 字段。新增字段会
   穿过 GUI↔agent 的 append-only bincode wire format，必须 bump
   `PROTOCOL_VERSION`。

在这些完成之前，声称 `pointer`/`report_rate`/`lighting` 能力只会渲染出无法读写
设备的面板，比一张仅身份的设备卡片更糟。

## 映射摘要

完整的功能清单与一一映射维护在 Darmoshark 驱动仓库的
`docs/OPENLOGI-FEATURE-MAP.md`。简表：

| Darmoshark 功能 | OpenLogi 界面 | 状态 |
|---|---|---|
| 身份、设备卡片、别名 | standalone inventory | 本分支 |
| 电量/充电 | `StandaloneDevice` + inventory 读取 | P3 |
| 固件/设备信息 | `StandaloneDevice` + model info | P3 |
| 当前 DPI 读写 | 现有 DPI IPC | P2 |
| 回报率 | 现有 report-rate IPC | P2 |
| LOD/角度 | OpenLogi 无面板 | P3 |
| 静态/呼吸/循环灯效 | 键盘 RGB 面板是 HID++ 专用 | P3 |
| 按键重映射 | HID++ ReprogControls / gaming tables | 部分；raw action 表未知 |
| 板载 profile 切换 | 无设备 profile IPC | P4 |
| SmartShift、拇指轮、触觉、Easy-Switch | — | 不适用 |

## 验证

```sh
cargo fmt --all -- --check
cargo test -p openlogi-device-registry darmoshark
cargo test -p openlogi-device darmoshark
cargo test -p openlogi-device standalone
cargo clippy -p openlogi-device-registry -p openlogi-device --all-targets -- -D warnings
cargo run -p openlogi -- list
```

插入无线接收器后，`openlogi list` 应包含名为
`Darmoshark M3S Pro (2.4 GHz)` 的设备记录。2026-11-05 已用一个调用
`openlogi_hid::enumerate_standalone()` 的最小测试程序验证：它只返回一条记录，
`kind=Mouse`、`driver=darmoshark`、`vid=1915 pid=0723 usage=ff0a:0001`、
online，并且能力集合是测量到的空集。不会执行任何写入，枚举也不需要设备权限。

## 范围与风险

OpenLogi 是 Logitech HID++ 的伴侣应用。注册非 Logitech 厂商属于产品范围
扩展，而不是中性的驱动新增。本分支是用于验证“standalone 驱动接缝能否承载
Darmoshark 协议”的 PoC，不代表上游会接受该设备族。codec 与身份模块彼此隔离，
因此这个实验可以整体丢弃而不触碰 HID++ 层。
