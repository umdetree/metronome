# ASIO 多客户端验证（Windows）

`asio-probe` 是独立实验工具，不改动节拍器配置、不监控播放吉他干声、不录音。目标是验证同一个驱动是否允许 REAPER 和本程序同时采集输入、同时输出。当前工具不做起音检测、波形绘制或节奏评分。

## 构建

默认节拍器不启用 ASIO，也不需要 ASIO SDK。仅构建探测工具时启用 `asio-probe` feature：

```powershell
.\scripts\build-asio-probe.ps1
```

要求 x64 Windows、Rust MSVC 工具链、Visual C++ 构建工具及 `libclang.dll`。脚本尝试寻找 LLVM 或 MSYS2 中的 libclang；可显式指定：

```powershell
.\scripts\build-asio-probe.ps1 -Libclang 'C:\path\to\llvm\bin' -AsioSdk 'C:\path\to\asio-sdk'
```

未指定 SDK 时，`asio-sys` 的构建脚本会从 Steinberg 下载 SDK 到隔离的临时构建目录。SDK 不纳入 Git；脚本不安装系统软件、不修改永久环境变量。ASIO SDK 有自己的许可条款，发布 ASIO 二进制前需核查所用 SDK 文件的许可要求；此实验不代表已满足公开分发要求。

MinGW libclang 必须指定 `--target=x86_64-pc-windows-msvc`，否则 C++ 结构布局可能与 MSVC 不一致。构建脚本已经设置此参数，并在结束后恢复环境变量。

## 准备 REAPER

1. 选择 `Yamaha Steinberg USB ASIO`，吉他输入启用监听，效果器正常工作。
2. 使用 UR12 耳机输出，先降低耳机音量。硬件直接监听建议关闭，避免把干声误认为 REAPER 湿声。
3. 在音频偏好设置中关闭 **Close audio device when stopped and application is inactive**（具体文字可能随版本不同）。否则切到终端时 REAPER 可能释放驱动，产生假阳性。
4. 不改变采样率和缓冲大小。探测工具读取驱动当前采样率，使用驱动默认/首选缓冲大小，不主动请求新数值。

工具默认延迟 10 秒才打开驱动，方便切回 REAPER；运行结束后会自动释放资源。不要用“REAPER 进程仍在”代替“REAPER 音频引擎仍在工作”的验证。

## 阶段 1：输入共享

```powershell
.\target\debug\asio-probe.exe --seconds 20 --delay 10
```

默认选择输入通道 2（UR12 的吉他输入通常为第二通道）；工具同时显示所有输入通道的峰值。如接线不同，可用 `--input-channel 1`。

在测试期间交替拨弦和停顿，确认：

- REAPER 耳机湿声持续正常，效果器仍然生效。
- 探测程序每秒都有输入回调，对应通道电平随拨弦变化。
- 没有设备占用错误、明显卡顿或 REAPER 失去音频。

信号计数使用 -60 dBFS 阈值，只是辅助判断；噪声也可能超过阈值，不能据此单独认定检测到了吉他。

## 阶段 2：共同输出

输入共享通过后运行：

```powershell
.\target\debug\asio-probe.exe --duplex --seconds 20 --delay 10
```

会以 120 BPM 向所有输出通道播放短点击声，默认峰值上限 0.03，**不输出输入的吉他信号**。如需调整，可用 `--bpm 100 --level 0.02`；音量参数限制为 0–0.1。

确认耳机里同时存在 REAPER 的效果器湿声和本工具的点击声，双方都未停音。驱动能打开、输入能共享，并不保证同一输出通道也能混合。

## 其他命令

```powershell
.\target\debug\asio-probe.exe --help
.\target\debug\asio-probe.exe --list
.\target\debug\asio-probe.exe --describe --delay 10
```

`--list` 只枚举注册的名称，不逐个初始化无关设备。`--describe` 仅打开指定驱动并查询配置，不创建音频缓冲，不能作为流共享成功的证据。

## 结果解释

- 输入和共同输出都成功，且 REAPER 持续工作：路线一在当前驱动、设备设置下具备可行性，随后仍需长时间稳定性和延迟校准测试。
- 能采集但 REAPER 停音：不算多客户端成功。
- 输入成功但共同输出失败：只能确认输入共享，需另找节拍输出路径或改用 REAPER 插件路线。
- 打不开驱动：记录错误后对比 REAPER 完全退出时的结果，区分占用限制与工具/驱动问题。

回调间隔不等于输入延迟或往返延迟；目前 CPAL 的 ASIO 后端不能上报所有驱动错误或掉音。工具输出是诊断线索，不是专业测量结论。

## 本机实测记录（2026-10-10）

设备为 UR12，驱动名称为 `Yamaha Steinberg USB ASIO`，使用本工具默认的输入模式：

| 条件 | 结果 |
| --- | --- |
| REAPER 运行时，第 1 次 | 驱动加载失败：`hardware input or output is not present or available`，尚未创建输入流 |
| REAPER 运行时，第 2 次 | 同样的驱动加载错误 |
| 用户完全退出 REAPER 后 | 成功启动输入，20/20 秒有持续回调，第二通道有明显吉他信号 |

独占模式报告：2 个输入通道、44100 Hz、I32、实际每次回调 512 帧（约 11.61 ms），共 1720 个输入回调。第一通道峰值约 -84 dBFS，第二通道拨弦峰值最高约 -17.4 dBFS。最大输入回调间隔约 47.32 ms；该值包含启动期及进程调度影响，不能当作往返延迟或准确的掉音计数。

结论：探测工具在独占条件下能正常采集 UR12 吉他输入，但当前配置未通过 REAPER 并行输入测试。现象与驱动占用限制相符；不能据此断言所有 Yamaha 驱动版本都不支持多客户端。因为输入共享尚未通过，**没有执行共同输出测试，也未证明共享输出可用**。后续可调查驱动多客户端设置，或转向由 REAPER 独占 ASIO 的插件路线。
