# zero-desktop 音频清洗独立成左侧菜单 设计

> 设计文档（**已实现**，2026-06-15；方案 A）。在已交付的「音频清洗」功能
> （见 [docs/2026-06-15-zero-desktop-audio-cleanup/design.md](../2026-06-15-zero-desktop-audio-cleanup/design.md)）
> 基础上，把入口从「语音识别」页顶部内嵌卡片，迁为左侧导航的**独立页签**。
> 纯前端重组，**后端零改动**。

---

## 1. 背景与动机

迁移前，音频清洗以 `<AudioCleanCard />` 内嵌在「语音识别」页（[SpeechPage.tsx](../../crates/zero-desktop/ui/src/modules/speech/SpeechPage.tsx)）
主区顶部，压在 segment 识别结果列表之上。问题：

- **语义不属于这里**：清洗的输入是「任意本地音/视频文件」，与当前语音识别会话（远程麦克风流、
  segment 列表）完全解耦——卡片注释自己就写了「与 segment 识别流解耦」
  （迁移后位于 [audio-clean/AudioCleanCard.tsx:33](../../crates/zero-desktop/ui/src/modules/audio-clean/AudioCleanCard.tsx#L33)；
  迁移前在 `speech/components/AudioCleanCard.tsx`，本次已删除，见 §5）。
- **挤占识别页空间**：清洗卡占掉识别结果区顶部一大块，录音/识别时是干扰。
- **不可发现**：作为页内卡片，没有稳定入口，用户得先进语音识别页才能用。

**目标**：把音频清洗提为左侧导航一级菜单（与「语音识别」「网络策略」同级），成为独立页面；
从语音识别页移除该卡片。

## 2. 现状盘点（要动 / 不动）

| 层 | 文件 | 现状 | 本次 |
|---|---|---|---|
| 后端 command | `crates/zero-desktop/src/modules/speech/commands/clean.rs` | `speech_clean_recording` / `speech_pick_audio_file` / `speech_open_in_folder`，走 toolkit-server 代理，文件来源，不覆盖原文件 | **不动** |
| toolkit-server 代理 | `crates/toolkit-server/src/routes/audio.rs` | `/api/web/audio/clean` | **不动** |
| 前端 API/类型 | `ui/src/modules/speech/api/tauri-client.ts` | `SpeechAPI.cleanRecording/pickAudioFile/openInFolder` + `CleanOptions`/`CleanedRecording` 类型 | 见 §4 方案选择 |
| 前端 UI 组件 | `ui/src/modules/speech/components/AudioCleanCard.tsx` | 完整清洗卡片 | **迁移** |
| 路由 | `ui/src/App.tsx` | 无清洗路由 | **新增** |
| 导航 | `ui/src/shared/ShellLayout.tsx` | `navItems` 5 项 | **新增 1 项** |
| 语音识别页 | `ui/src/modules/speech/SpeechPage.tsx` | 内嵌 `<AudioCleanCard />` | **移除内嵌** |

后端命令名维持 `speech_*` 前缀不改（已在 `main.rs` 注册、已实测），避免无谓的注册/契约改动——
命名前缀不影响功能，留作历史包袱可接受。

## 3. 路由与导航设计

- **路由路径**：`/audio-clean`（`App.tsx` 新增一条 `<Route path="audio-clean" … />`）。
- **导航项**：插在「语音识别」之后（功能相邻），label「音频清洗」。
- **图标**：用 `lucide-react` 的 `Wand2`（与卡片内现用的 wand 隐喻一致）；
  备选 `AudioWaveform` / `Sparkles`。
- **落地页布局**：独立页不再与 segment 列表并排，改为**居中、限宽**（如 `max-w-3xl mx-auto`），
  顶部加页标题区（标题「音频清洗」+ 一句副文「去 BGM / 降噪 / 响度归一，处理任意本地音/视频文件」），
  下接清洗卡片。

## 4. 组件与目录组织（两方案，推荐 A）

### 方案 A（推荐）：新建 `modules/audio-clean/` 模块，业务/API 解耦（UI 原子暂复用 speech）

> 解耦范围限定为**业务逻辑 + 清洗 API/类型**：页面、卡片、`CleanAPI` 全部独立。
> 但卡片内部的 UI 原子（`Button`/`Dropdown`/`Switch`/`Icon`）暂继续从 `speech/components/ui/*`
> 引用（最小风险，见 §5 末），因此**不是 100% 零依赖**——把 UI 原子上提到 `shared/ui/` 留作
> 后续清理项（§6）。

新建目录 `ui/src/modules/audio-clean/`：

- `AudioCleanPage.tsx` —— 页面外壳（标题区 + 居中限宽容器 + 渲染卡片）。
- `AudioCleanCard.tsx` —— 由 `modules/speech/components/AudioCleanCard.tsx` **移入**，
  内容基本不变。
- `api/clean-client.ts` —— 把清洗相关的 3 个 API（`cleanRecording`/`pickAudioFile`/`openInFolder`）
  与 `CleanOptions`/`CleanedRecording` 类型从 `speech/api/tauri-client.ts` **迁出**，独立成
  `CleanAPI`。这些本就是清洗专属、与语音识别无关。

理由：模块边界清晰，audio-clean 不再寄生在 speech 下；后续若加「批量清洗」「历史记录」等只在本模块演进。
代价：一次性多动几个文件 + 调整 import。

### 方案 B（轻量）：仅搬页面，API 留在 speech

- 新建 `modules/audio-clean/AudioCleanPage.tsx`，但 `AudioCleanCard` 留在原位，
  Page 跨模块 `import { AudioCleanCard } from '../speech/components/AudioCleanCard'`。
- `tauri-client.ts` 的清洗 API 不动。

理由：改动最小、风险最低。代价：audio-clean 仍依赖 speech 模块，边界含糊，清洗 API 永久挂在 `SpeechAPI` 名下。

> **推荐 A**：这次本就是「让它独立」，顺手把模块边界理清，避免日后清洗功能继续在 speech 里长肉。
> 若想先快速见效，可先做 B、后续再收敛——但通常一步到位更省事。

## 5. 改动清单（方案 A）

1. **新建** `ui/src/modules/audio-clean/api/clean-client.ts`：从 `speech/api/tauri-client.ts`
   迁入 `cleanRecording`/`pickAudioFile`/`openInFolder` + `CleanOptions`/`CleanedRecording`，
   导出 `CleanAPI`。
2. **新建** `ui/src/modules/audio-clean/AudioCleanCard.tsx`：从 speech 移入，import 改指
   `./api/clean-client` 的 `CleanAPI`。
3. **新建** `ui/src/modules/audio-clean/AudioCleanPage.tsx`：标题区 + 居中容器 + `<AudioCleanCard />`。
4. **删除** `ui/src/modules/speech/components/AudioCleanCard.tsx`。
5. **改** `ui/src/modules/speech/api/tauri-client.ts`：移除已迁出的清洗 API/类型
   （确认 speech 内无其它引用）。
6. **改** `ui/src/modules/speech/SpeechPage.tsx`：删 `import { AudioCleanCard }` 与
   原内嵌的 `<AudioCleanCard />`（**迁移前**位于该文件主区 `<div className="…flex flex-col gap-3">` 顶部，
   约旧第 461 行；现已移除，当前文件无此引用）。
7. **改** `ui/src/App.tsx`：import `AudioCleanPage` + 新增 `<Route path="audio-clean" element={<AudioCleanPage />} />`。
8. **改** `ui/src/shared/ShellLayout.tsx`：`navItems` 在「语音识别」后插
   `{ to: '/audio-clean', icon: Wand2, label: '音频清洗' }`，并 import `Wand2`。
9. **改（附带，非功能代码）** `.claude/launch.json`：新增一个 `zero-desktop-ui`（`npm run dev`，端口 1420）
   预览配置，纯属本地验证/预览便利，**与功能逻辑无关**，不影响打包产物。列此仅为如实记录本次
   工作树改动；若不需要前端预览可省略此项。

`Icon`（speech 自有的 `components/ui/Icon`）、`Button`/`Dropdown`/`Switch` 这些卡片内部依赖：
方案 A 下需一并迁移或共享。建议把 `components/ui/*` 视作**共享 UI 原子**，要么保留跨模块 import
（`../../speech/components/ui/*`，可接受），要么这次顺带上提到 `ui/src/shared/ui/`（范围更大，可不做）。
**最小风险选择**：卡片内部 UI 原子继续从 speech 引用，不上提——只迁清洗专属逻辑。

## 6. 不在本次范围

- 后端任何改动（命令、代理、落盘策略）。
- 清洗能力本身（选项、上游契约）。
- 把 `speech_*` 命令改名为 `audio_clean_*`（纯改名、收益低、易引入回归，略）。

### 后续任务（独立排期）

- **UI 原子上提到 `shared/ui/`**：把 `Button`/`Dropdown`/`Switch`/`Icon` 从 `speech/components/ui/`
  迁到 `ui/src/shared/ui/`，speech 与 audio-clean 同时改指，audio-clean 即与 speech **完全零依赖**。
  影响面较大（speech 多处引用），故不并入本次，单列为清理项。

## 7. 验收

**功能/UI**

1. 左侧导航出现「音频清洗」项，点击进入 `/audio-clean`，页面居中限宽、有标题区与清洗卡片。
2. 「语音识别」页顶部不再有清洗卡片，识别结果区回到满高。
3. 选文件 → 设选项 → 开始清洗 → 成功落 `<stem>.cleaned.<fmt>`、显示阶段/响度、可打开文件夹——
   行为与迁移前完全一致。

**前端门槛**

4. `npm run build`（= `tsc --noEmit && vite build`）通过，无 TS 报错、无悬空 import。

**仓库级门槛**（CLAUDE.md 编码约定；本次改动为纯前端，无 `.rs` 改动，下列应自然通过，
但仍须实跑确认未被 IDE/格式化连带改动 Rust 文件）：

5. `cargo fmt --check` 通过（无格式漂移）。
6. clippy —— `toolkit-desktop` 与 `zero-desktop` 均为 Tauri crate，须按环境二选一：
   - **有 Tauri 工具链**：`cargo clippy -p zero-desktop -- -D warnings`（本次唯一相关 crate，直接针对它）。
   - **无 Tauri 工具链**：zero-desktop 无法编译，跳过它；
     `cargo clippy --workspace --exclude toolkit-desktop --exclude zero-desktop -- -D warnings`
     守住其余 crate（与本次前端改动无关，仅防误改）。
7. test —— 同上二选一：有 Tauri `cargo test -p zero-desktop`；
   无 Tauri `cargo test --workspace --exclude toolkit-desktop --exclude zero-desktop`。

> 说明：本次仅改 `.tsx/.ts` + `App.tsx`/`ShellLayout.tsx` + `.claude/launch.json` + 文档，
> 未触碰任何 Rust 源；5~7 的意义是守住「实现已完成」的判定不低于仓库标准、并拦截误改。
