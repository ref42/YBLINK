# YBLINK HPM5301 固件

中文为默认 README。英文版见 [README.en.md](README.en.md)。

YBLINK（Yet Better Link）是运行在 HPM5301 上的纯 Rust CMSIS-DAP v2 调试器固件。当前稳定版本使用 USB High Speed、FGPIO 软件模拟 SWD/JTAG，并提供一个 CDC ACM USB 转 UART 串口桥。

## 当前 USB 信息

| 字段 | 值 |
| --- | --- |
| VID:PID | `1209:5301` |
| Product | `YBLINK CMSIS-DAP` |
| Serial | `YBLINK` |
| Firmware version | `0.1.0` |

## 功能状态

| 功能 | 状态 |
| --- | --- |
| CMSIS-DAP v2 | 可用，USB Bulk 传输 |
| SWD | 可用，默认 8 MHz |
| JTAG | 可用 |
| CDC ACM 串口桥 | 可用，默认 1,000,000 baud |
| 活动指示灯 | PA10，低电平点亮 |

固件会把 probe-rs 默认请求的 1 MHz 映射为 8 MHz。为了兼容 STM32H723ZGT6，目前所有大于 8 MHz 的 SWD/JTAG 请求都会被限制为 8 MHz。

## 引脚映射

固件使用 HPM5301EVKLite 的 J3 GPIO 排针。目标板需要正常供电，并且必须和 YBLINK 共地。

### SWD

| YBLINK 信号 | HPM5301 引脚 | J3 引脚 | 目标板信号 |
| --- | --- | ---: | --- |
| SWCLK | PA27 | 23 | SWCLK |
| SWDIO | PA28 | 21 | SWDIO |
| nRESET | PB10 | 26 | NRST |
| GND | GND | 25/30/34/39 | GND |

普通 SWD 模式下，PA26 和 PA29 不需要连接。

### JTAG

| YBLINK 信号 | HPM5301 引脚 | J3 引脚 | 目标板信号 |
| --- | --- | ---: | --- |
| TCK | PA27 | 23 | TCK |
| TMS | PA28 | 21 | TMS |
| TDI | PA29 | 19 | TDI |
| TDO | PA26 | 24 | TDO |
| nRESET | PB10 | 26 | NRST |
| GND | GND | 25/30/34/39 | GND |

### 串口桥

| YBLINK 信号 | HPM5301 引脚 | J3 引脚 | 外部信号 |
| --- | --- | ---: | --- |
| UART0 TXD | PA00 | 36 | 目标 RXD |
| UART0 RXD | PA01 | 38 | 目标 TXD |
| GND | GND | 25/30/34/39 | GND |

串口环回测试可以直接连接 PA00/J3-36 到 PA01/J3-38。

## 构建

第一次构建前安装 RISC-V target：

```bash
rustup target add riscv32imafc-unknown-none-elf
```

构建 release 固件：

```bash
cargo build -p yblink --release
```

输出 ELF：

```text
target/riscv32imafc-unknown-none-elf/release/yblink
```

## 烧录 YBLINK 固件

使用外部调试器给 HPM5301 烧录：

```bash
probe-rs download --chip HPM5301 --protocol jtag target/riscv32imafc-unknown-none-elf/release/yblink
probe-rs reset --chip HPM5301 --protocol jtag
```

仓库也保留了一个预编译固件：

```text
firmware/yblink
```

## 使用 YBLINK 烧录目标芯片

查看 probe：

```bash
probe-rs list
```

烧录目标芯片示例：

```bash
probe-rs download --chip STM32F405RG --speed 8000 --probe 1209:5301:YBLINK app.elf
```

如果不指定 `--speed`，probe-rs 的默认 1 MHz 请求会被固件映射为 8 MHz。

## 自制硬件

| <img src="hardware_altium_project/YBLINK_TOP.png" width="480"> | <img src="hardware_altium_project/YBLINK_BOTTOM.png" width="480"> |
| :-----------------------------------------------------: | :-----------------------------------------------------: |

自制板已经手焊并测试通过。原理图、PCB 和 Gerber 文件在 [hardware_altium_project](hardware_altium_project) 目录中。用于烧录 YBLINK 固件的 5 个过孔从左到右是：TMS、TCK、TDI、TDO、TRST。

| <img src="images/top.jpg" width="480"> | <img src="images/bottom.jpg" width="480"> |
| :-----------------------------------------------------: | :-----------------------------------------------------: |
