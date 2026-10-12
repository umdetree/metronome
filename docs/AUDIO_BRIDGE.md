# REAPER 干声桥接：CLAP 插件 + TUI（实验版）

本阶段只完成干声透传、跨进程接收、电平和波形。**没有起音检测、节奏评分、插件内节拍或延迟校准**。REAPER 独占其音频设备，TUI 接收音频不需要打开 ASIO。

## 构建与安装（Windows x64）

```powershell
.\scripts\build-bridge-plugin.ps1
```

输出 `dist/metronome-dry-audio-bridge.clap`。这是 Rust 编译的原生 CLAP 插件，不是 VST3，不需要 ASIO SDK。

安装到当前用户的 CLAP 标准目录：

```powershell
.\scripts\build-bridge-plugin.ps1 -Install
```

安装路径为 `%LOCALAPPDATA%\Programs\Common\CLAP`，无需管理员权限。脚本只在明确传入 `-Install` 时安装，不修改 REAPER 工程或偏好设置。更新之前关闭使用该插件的 REAPER 实例，避免覆盖已加载的 DLL。随后在支持 CLAP 的 REAPER 中重新扫描插件或重启，搜索 **Metronome Dry Audio Bridge**。

Linux/macOS 可用 `cargo build --release -p metronome-bridge-plugin` 构建相应动态库，但本阶段只在 Windows 验证了构建和本机 IPC；其他平台的 CLAP 打包及宿主测试尚未验证。

## 放置位置

推荐：

```text
吉他输入轨（输入监听开启）
  UR12 输入 → Metronome Dry Audio Bridge → Neural DSP → Master → 耳机
```

用户原本的 **FX: Monitoring** 用法也可以保留：把桥接插件放在 Monitoring FX 的最前面，排在所有 Neural DSP 之前。注意：

- 此时采集的是进入监听效果链的混合信号，不保证是纯吉他输入。
- 轨道 FX、Master FX 等更早的处理已经发生，桥接插件无法还原其干声。
- 其他轨道、REAPER 节拍声或伴奏也可能混入。
- 验证电平和波形时可暂时只让吉他发声；以后做起音检测和评分，推荐放在独立吉他轨效果链首位。

若切到终端后音频停止，请检查 REAPER 的 **Close audio device when stopped and application is inactive** 选项。关闭它，或保持宿主持续处理音频；TUI 不会擅自修改这个设置。

插件是无自定义 GUI 的双声道透传效果。兼容宿主的 32 位或 64 位音频缓冲，输出保持原精度；桥接分析数据统一转换为 `f32`。不对输入增益、延迟、音色或监听混音做修改。

## 使用 TUI

```powershell
cargo run --release --bin metronome
```

按 `I` 打开输入监视页：

- 唯一在线实例自动连接；存在多个实例时按 `N` 选择，实例 ID 含宿主进程 ID 和唯一标识。
- `C` 切换所查看的插件输入通道。这里是**插件输入通道**，不直接等同于声卡硬件通道；单声道吉他可能由 REAPER 路由到插件左右两边。
- `R` 重新扫描/连接，`Esc` 或 `I` 返回原界面，`Q` 退出。
- 电平以 dBFS 显示，波形为最近约 2 秒的最小/最大包络，不是逐样本示波器。
- 显示积压、连接之后丢块数和检测到的音频间断/重置次数。
- 没开 TUI、TUI 卡住或退出都不影响插件透传。插件满队列时丢弃新的分析数据，不等待接收端。

独立节拍器仍使用原 Rodio 输出，**不与 REAPER 音频时间轴同步**。输入页允许空格控制原节拍器，但其输出设备可能受驱动占用限制；这一阶段请优先验证输入链路，不把波形与独立节拍当作准确评分依据。

## 不启动 TUI 的诊断

```powershell
cargo run --release --bin bridge-probe -- --list
cargo run --release --bin bridge-probe -- --seconds 10
```

多个插件实例时使用 `--source ID`。每个实例只允许一个接收者：诊断前退出 TUI，避免接收者锁冲突。TUI 后台会自动连接唯一实例，即使尚未打开输入页。

## 线程与性能

```text
REAPER 音频回调 → 固定容量共享内存队列
                  ↓
TUI 接收/分析线程（约 2 ms 轮询，实际调度可能更慢）
                  ↓
最新显示快照（约 30 Hz）→ 终端绘制
```

- 音频回调没有文件操作、分配、等待锁或等待接收端，只透传并写有界音频块。
- 分析端生成固定数量波形包络；显示落后不积累无限消息。
- 显示快照用普通短锁，插件完全不涉及此锁；TUI 读取使用 `try_lock`。
- 固定 32 个槽，每槽最多 1024 帧、2 通道。更大的宿主块拆成多个槽。
- CPU 占用和长时间稳定性仍需在实际宿主中测量，不能凭自动测试保证实时性能。

## 协议 v1

实现与接收 API 位于 `crates/audio-bridge`。头部包含 magic、版本、总长度、生命周期状态、generation、采样率、通道数、读写序号及丢块计数。音频块包含：

| 字段 | 意义 |
| --- | --- |
| sequence | 每次尝试发送递增；丢块后不连续 |
| generation | 激活、重新开始、reset 或检测到宿主时间跳转时变化 |
| position | 插件本次激活以来的累计采样帧位置，不等同于曲目位置 |
| steady | 宿主提供的 CLAP steady_time，仅在有效标志存在时使用 |
| transport_seconds | 宿主曲目时间，明确标记是否有效及是否正在播放 |
| frames/channels/sample_rate | 有效音频尺寸与采样率 |
| samples | IEEE `f32`，帧交错顺序 |

使用同机、同架构的原生端序与 64 位原子操作，固定 `repr(C)` 布局，发布/消费通过 acquire/release 顺序同步。共享内存中没有 `Vec`、字符串、引用或跨进程指针。读取库隐藏底层布局，不同版本/尺寸拒绝连接。

这是协作型本机 IPC，不是面向不可信进程的安全隔离协议。文件位于用户本地数据目录的 `audio-bridge-v1` 下；所有生产者创建独立映射，绝不截断/复用已发布文件。生产者和接收者分别持有 OS 文件锁，异常退出会释放锁；发现时忽略没有活动生产者的遗留文件。正常退出尽量删除文件，Windows 上仍被映射的文件可能留下，清理前应关闭所有相关宿主与接收进程。

采样位置、宿主曲目时间和进程接收时刻不是同一概念。尚未补偿音频设备、效果链、宿主轨道延迟或输入检测延迟；当前 API 提供时间基础，不承诺采样级演奏偏差测量。

## 验证

```powershell
cargo test --workspace --locked
cargo clippy --workspace --locked --all-targets -- -D warnings
cargo fmt --all --check
```

自动测试覆盖真实跨进程映射传输、单接收者锁、队列满时不覆盖旧音频、协议版本拒绝、32/64 位透传与 in-place 缓冲、接收者停顿时仍正常透传、波形容量与断流检测。仍需进行长时间运行、CPU 占用及更多宿主/路由的验证。

### 本机 REAPER 验证（2026-10-13）

- 插件已安装到用户 CLAP 目录；用户确认 REAPER 加载成功、效果器湿声正常。
- 外部 `bridge-probe` 在 10 秒内收到 861 个音频块，采样率 44100 Hz，左右通道都有随拨弦变化的电平。
- 每秒报告的队列积压为 0；测试期间丢块总计数保持不变，没有新增队列丢块。
- 开始时已有 3953 个历史丢块：插件运行而接收者尚未连接时，固定队列满后主动丢弃分析数据。这不会阻塞宿主音频。诊断和 TUI 均以连接之后的增量显示丢块数。
- 此结果验证了插件到外部进程的接收链路，不等于完成延迟校准或证明长期无掉音。TUI 波形的实际终端观感仍需用户检查。
