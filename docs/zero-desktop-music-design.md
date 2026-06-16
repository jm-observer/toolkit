# zero-desktop 音乐播放模块 设计文档

> 状态：设计稿（待评审）· 作者：fengqi · 日期：2026-06-16
> 关联：[zero-desktop 模块化架构](../crates/zero-desktop/)（english / speech / cookie / codeloop 同款套路）

## 1. 目标与范围

在 zero-desktop 里集成**本地音乐播放**：

- **曲库来源**：用户选定的本地文件夹，后端递归扫描音频文件。
- **形态**：独立模块页「音乐」（曲库浏览 / 播放列表）+ **常驻迷你播放器**（底栏，跨页不中断播放）。
- **音质目标**：尽量「原始、逼真」——预留**高保真后端**（bit-perfect / hi-res），但默认走简单后端。

非目标（本期不做）：在线流媒体 / 版权曲库、歌词滚动、音效均衡器、跨设备同步。

## 2. 关键技术决策：浏览器解码 vs 后端解码（音质分析）

这是本设计的核心争点。结论先行：

> **解码阶段不丢音质；丢音质的是"输出阶段"（重采样 + 系统混音器）。**
> 普通文件 + 普通设备：两种方案**听感无差异**。
> 追求 bit-perfect / hi-res 原始音质：**必须走原生后端（WASAPI 独占 / ASIO），浏览器 `<audio>` 永远做不到**。

### 2.1 解码本身（FLAC/MP3 → PCM）

- **无损格式（FLAC / WAV / ALAC）**：解码是确定性的，任何正确解码器输出的 PCM **逐位相同**。浏览器和 Symphonia 没有区别。
- **有损格式（MP3 / AAC / Ogg）**：不同解码器的 PCM 可能有极微小差异，但**不可闻**，且不是"音质高低"问题。

**所以解码器选谁都不影响音质。**

### 2.2 输出阶段——音质真正的分水岭

| 环节 | 浏览器 `<audio>` / Web Audio | 原生（cpal/WASAPI/ASIO） |
|---|---|---|
| 采样率 | **强制重采样**到 AudioContext 速率（常见 48kHz 固定）；hi-res 96/192kHz 文件被降采样 | 可**跟随文件原始采样率**切换设备 |
| 系统混音器 | 必经 Windows 共享混音器（WASAPI shared），再被系统音量/音效处理 | **独占模式可绕过混音器**，PCM 直达 DAC |
| 位深 | 内部 32-bit float，输出经系统混音降到 16/24bit | 可 bit-perfect 输出文件原始位深 |
| 重采样质量 | 浏览器内置（够用，非发烧级） | 可选 Rubato sinc（发烧级） |
| Gapless 无缝 | ❌ HTML5 `<audio>` 切歌有间隙 | ✅ Symphonia + 自管缓冲可无缝 |
| ReplayGain / 抖动 | ❌ | ✅ 可控 |

**一句话**：浏览器播放永远在系统共享混音器后面，做不到 bit-perfect；对 16bit/44.1kHz 普通文件经普通音箱**听不出差**，但对 hi-res 文件 + 好 DAC，原生独占模式有**可闻**的提升（避免二次重采样）。

### 2.3 可用的 Rust 仓库（原生高保真链路）

| crate | 角色 | 说明 |
|---|---|---|
| **`symphonia`** | 解码 + 解封装 | 纯 Rust，支持 FLAC/MP3/AAC/ALAC/Ogg/Vorbis/WAV/MP4 等，**逐位精确**、支持 gapless。事实标准。 |
| **`cpal`** | 跨平台音频输出 | Windows=WASAPI、macOS=CoreAudio、Linux=ALSA。可选 `asio` feature（需 ASIO SDK）。**默认共享模式**；近期加了 WASAPI 实时线程优先级。 |
| **`rodio`** | 高层播放器 | 基于 cpal，解码默认用 symphonia，`.with_gapless(true)` 开无缝。**上手最快**，但重采样是基础线性、非 bit-perfect。 |
| **`wasapi`** | WASAPI 直接绑定 | 若要 Windows **独占模式 / event-driven**（cpal 对独占支持不完整），用它（CamillaDSP 作者维护）。bit-perfect 必需。 |
| **`rubato`** | 高质量重采样 | 仅当确需重采样时用（sinc，发烧级）。 |
| **`lofty`** | 标签元数据 | 解析 title/artist/album/duration/封面。与播放链路解耦。 |

### 2.4 选型建议：分层、可切换后端

把「播放引擎」抽象成接口，**默认浏览器、可选原生**：

- **MVP / 默认后端 = 浏览器 `<audio>`**：零原生依赖、跨平台稳、实现快；满足绝大多数听感需求。
- **高保真后端（可选，二期）= Symphonia + cpal**（Windows 进一步上 `wasapi` 独占模式）：在设置里开「高保真/独占模式」开关时启用，PCM 原样直达 DAC。
- 两个后端实现同一个前端 `Player` 接口（play/pause/seek/...），UI 与曲库扫描完全复用。

> 决策理由：发烧级 bit-perfect 要写音频线程、缓冲、seek、设备枚举、独占模式协商，工作量大且只对 hi-res+好硬件可闻。先用浏览器把整条产品链路跑通，把原生后端作为**可插拔增强**，避免一上来背上原生音频的复杂度。

## 3. 架构总览

```
┌─ UI (React) ─────────────────────────────────────────────┐
│  MusicPage（曲库/列表）   MiniPlayer（底栏常驻）           │
│              └──────┬───────────┘                         │
│            PlayerContext（唯一 <audio> / 或原生后端代理）   │
└──────────────────────────┬───────────────────────────────┘
              invoke         │  convertFileSrc(asset://)
┌──────────────────────────┴───────────────────────────────┐
│  Rust: modules/music                                      │
│   music_pick_folder / music_scan / music_grant_dir        │
│   （扫目录 + lofty 元数据 + 运行时授权 asset scope）        │
│   [二期] native_play/pause/seek（symphonia+cpal 音频线程）  │
└───────────────────────────────────────────────────────────┘
```

**核心原则**：默认后端下，Rust 端**无状态、无音频依赖**——只做"扫目录 + 出元数据 + 授权文件访问"。播放在前端。

## 4. 后端设计（Rust）：新增 `crates/zero-desktop/src/modules/music/`

### 4.1 状态与命令

```rust
pub struct MusicState {}   // 默认后端无状态；选中目录存 tauri-plugin-store

#[derive(Serialize)]
struct Track {
    path: String,              // 绝对路径，前端 convertFileSrc 用
    title: String,             // lofty 标签；缺失回退文件名
    artist: Option<String>,
    album: Option<String>,
    duration_secs: Option<f64>,
    has_cover: bool,
}

#[tauri::command] async fn music_pick_folder(app) -> Option<String>  // dialog 选目录 → 授权 → 返回路径
#[tauri::command] async fn music_scan(dir: String) -> Result<Vec<Track>, String>  // 递归扫描 + 元数据
#[tauri::command]       fn music_grant_dir(app, dir: String)         // 启动时对已存目录重新授权
#[tauri::command] async fn music_cover(path: String) -> Result<Option<String>, String>  // 可选：内嵌封面落临时文件
```

支持扩展名：`mp3 / flac / wav / m4a / aac / ogg / opus`。

### 4.2 关键点 —— 运行时授权 asset 协议

`tauri.conf.json` 的 `assetProtocol.scope` 是**静态**的（仅含 app 数据目录），用户自选音乐目录不在内，`convertFileSrc` 会被拦。必须在选目录 / 启动重授权时动态放行：

```rust
app.asset_protocol_scope().allow_directory(&dir, true);  // recursive
app.fs_scope().allow_directory(&dir, true);
```

启动时（`main.rs` 的 `.setup()`）从 plugin-store 读回上次选的目录并重授权，否则重启后放不了。

### 4.3 持久化

- 选中的文件夹路径：用现成的 **`tauri-plugin-store`**（与 english 存 KV 同套路），**不进 workspace、不建 SQLite**。
- 音乐文件**留在用户文件夹，不复制进 workspace**。

### 4.4 后端接线清单

1. `src/modules/mod.rs` → `pub mod music;`
2. `src/app_state.rs` → `AppState` 加 `pub music: Arc<MusicState>`
3. `src/main.rs` → `invoke_handler!` 注册命令；`.setup()` 里读 store 已存目录并 `allow_directory`
4. `Cargo.toml` → 加 `lofty`（MVP 可暂缓，先文件名当标题）

## 5. 前端设计（UI）：`crates/zero-desktop/ui/src/modules/music/`

### 5.1 全局播放状态 `PlayerContext.tsx`

- Provider 内持唯一一个隐藏 `<audio>`（默认后端），暴露 `play(track)/pause/next/prev/seek/setVolume` 与 `{current, queue, playing, progressSec, durationSec}`。
- 在 `App.tsx` 把 `<MusicPlayerProvider>` 包在 `<ShellLayout>` **外层** → 切路由 audio 不卸载，**播放不中断**。
- 后端可切换：接口对 UI 不变；二期原生后端时，这里把 `<audio>` 调用替换成 invoke 原生命令。

### 5.2 模块页 `MusicPage.tsx`（侧栏「音乐」入口）

- 顶部：当前文件夹 + 「选择文件夹」「重新扫描」。
- 主体：曲库列表（标题/歌手/时长/封面），搜索过滤，点击即 `play` 并设队列。
- 复用 `chat-summary` / `english` 的 Tailwind 卡片风格。

### 5.3 常驻迷你播放器 `MiniPlayer.tsx`

- 渲染在 `ShellLayout` 底部（现有 G10/Cookie/识别指示灯区下方）。
- 封面缩略 + 标题 + 播放/暂停/上一首/下一首 + 进度条 + 音量；全部从 `usePlayer()` 取，任何页面可控。

### 5.4 前端接线清单

1. `App.tsx` → import `MusicPage`，加 `<Route path="music" .../>`；外层包 `MusicPlayerProvider`
2. `shared/ShellLayout.tsx` → `navItems` 加 `{ to:'/music', icon: Music, label:'音乐' }`（lucide `Music` 图标）；底部渲染 `<MiniPlayer/>`
3. 新增 `ui/src/modules/music/`：`MusicPage.tsx` / `PlayerContext.tsx` / `MiniPlayer.tsx` / `api/tauri-client.ts`

## 6. 端到端播放路径（默认后端）

```
选文件夹 → music_pick_folder（授权 scope + 存 store）→ music_scan → Track[]
  → 点歌 → usePlayer().play(track)
  → audio.src = convertFileSrc(track.path) → 浏览器解码播放
  → MiniPlayer 跨页常驻控制
```

## 7. 分期落地

- **MVP（1 个 PR）**：后端扫目录（先文件名当标题，不引 lofty）+ 运行时授权；前端 Context + MusicPage 列表 + MiniPlayer 基本控件（播/暂停/上下首/进度/音量）。端到端能放，浏览器后端。
- **二期 · 元数据**：引 `lofty`，出 title/artist/album/duration/封面；列表按歌手/专辑分组、播放列表持久化、随机/循环。
- **三期 · 高保真后端（可选）**：Symphonia + cpal 原生播放引擎，设置加「高保真/独占模式」开关；Windows 上 `wasapi` 独占模式实现 bit-perfect，hi-res 文件跟随原始采样率，gapless 无缝。

## 8. 风险与注意

- **asset scope 授权**：忘了运行时 `allow_directory` 会导致选了目录也放不了（CSP/scope 拦截）——MVP 必测重启后仍可播。
- **大目录扫描**：几千首时 `music_scan` + lofty 解析要异步 + 可加进度；MVP 先同步小目录。
- **原生后端复杂度**：音频线程 / seek / 独占模式协商成本高，严格作为可选增强，不阻塞主链路。
- **跨平台**：默认浏览器后端天然跨平台；原生独占（WASAPI/ASIO）是 Windows 专属优化，macOS/Linux 走 cpal 默认即可。

---

### 参考

- [CPAL — Rust audio library (lib.rs)](https://lib.rs/crates/cpal)
- [RustAudio/cpal (GitHub)](https://github.com/RustAudio/cpal)
- [symphonia (crates.io)](https://crates.io/crates/symphonia)
- [rodio decoder docs](https://docs.rs/rodio/latest/rodio/decoder/)
- [WASAPI Exclusive vs Shared Mode 说明](https://aurisplayer.com/blog/wasapi-exclusive-guide.html)
