# Codeloop 会话选择器：按项目分组 + 同项目联动（设计文档）

> 适用范围：zero-desktop 复核循环（codeloop）的会话配对选择器
> （`crates/zero-desktop/ui/src/modules/codeloop/components/SessionPairPicker.tsx`）。
> 状态：设计已定稿，待实现。纯前端改造，**不动后端 / 不改 Tauri 命令**。

## 1. 背景与动机

Claude / Codex 的会话天然带工作目录（`cwd`），Claude 桌面端据此**按项目分组**展示
（streaming-speech / sim-trade / stock-trade / github-commit-info …）并用相对时间
（"1 分""28 分""20 小时""1 天"）排序。

zero-desktop 的 codeloop 选择器目前是两个**扁平**的可搜索 combobox（Claude 一个、Codex 一个），
项目名只是每行右侧的一个小 tag。用户希望对齐桌面端：**用"项目"组织会话清单**，提升可读性与
配对效率。

## 2. 关键约束（决定方案形态）

复核循环要求 **Claude 会话与 Codex 会话在同一个仓库**：`codeloop_start` 调
`validate::validate_three_way(claude.cwd, codex.cwd, target_path)`，三者不一致直接启动失败
（见 `crates/zero-desktop/src/modules/codeloop/mod.rs`）。

因此"项目"不仅是展示维度，更是**配对的钥匙**。把它显式化，可以在"选"的阶段就避免选出
跨仓库的无效组合，而不是等点了"启动"才报错。

## 3. 目标 / 非目标

**目标**
- L1：下拉按项目分组展示，相对时间，分组内/分组间按时间倒序；搜索筛选保留。
- L2：选定一边后，另一边把**同项目会话置顶高亮**、异项目会话下沉到"其他项目"分组（仍可选）。

**非目标**
- 不引入"项目优先"的整体重排（原 L3：先选项目再选会话）——留待后续。
- 不改后端 / 不新增 Tauri 命令 / 不改 `SessionSummary` 契约（`cwd`、`preview` 已具备）。
- 不做硬禁用：异项目会话仅弱化，不置灰禁选（容忍同仓不同路径写法、worktree 等边界）。

## 4. 既定细节（本次拍板）

| 点 | 决定 | 理由 |
|---|---|---|
| 时间格式 | **相对时间为主**（"刚刚 / N 分 / N 小时 / N 天 / N 月"），`title` 悬浮显示绝对时间 | 对齐桌面端观感，同时保留精确值可查 |
| 异项目会话 | **下沉到"其他项目"分组，仍可选、视觉弱化**（不禁用） | 避免误伤同仓不同路径写法/worktree；强引导而非强约束 |
| 后端 | 不动 | `cwd`/`preview` 已在 `SessionSummary` 中 |

## 5. 数据与现状

`SessionSummary`（`crates/agent-session/src/lib.rs`）已含：
- `cwd: string` — 会话工作目录（项目路径）。
- `preview: string` — 首条用户消息前若干字符。
- `title` / `status` / `updated_at` / `provider` / `id`。

前端 `SessionPairPicker.tsx` 已有 `projectName(cwd)`（取路径末段）、`tokenMatch`（分词筛选）、
`optionLabel`（`[status] preview` 回退 title/id）。本设计在此之上扩展。

## 6. L1：按项目分组 + 相对时间

### 6.1 分组与排序
- 把本侧 provider 的会话按 `projectName(cwd)` 分组（空 cwd → 归入"未知项目"组）。
- **组内**：按 `updated_at` 倒序。
- **组间**：按"组内最新会话的 `updated_at`"倒序（最近活跃的项目排前）。

### 6.2 渲染结构（下拉 `<ul>` 内）
```
📁 sim-trade
   [idle] 复核 runtime-db 读合并 ADR        1 分
   [idle] 审核 ADR 0022 runtime DB 读取整合  28 分
📁 github-commit-info
   [idle] 审阅 zero-desktop-music-design     2 小时
   …
```
- 分组头：📁(lucide `Folder`/`FolderOpen`) + 项目名，sticky 顶部（`position: sticky`），
  弱色、不可点击。
- 会话行：沿用现有"`[status] preview` + 右侧时间"，时间改相对时间，`title` 悬浮绝对时间 + 完整
  AI 标题。

### 6.3 相对时间
新增 `relativeTime(iso)`：
- `<60s` → "刚刚"；`<60min` → "N 分"；`<24h` → "N 小时"；`<30d` → "N 天"；否则 "N 月"。
- 无法解析 → 原样返回。
- 行上 `title` 属性给 `shortTime(iso)`（保留现有绝对格式）+ AI 标题，便于精确核对。
- **说明**：相对时间在下拉打开时按当前时刻计算一次即可（无需定时刷新——下拉是瞬时交互）。

### 6.4 筛选交互
- `tokenMatch` 不变（跨 preview/title/项目名/status/id 分词 AND 匹配）。
- 筛选后**空的项目组自动隐藏**；全部为空时显示"无匹配会话"。
- 输入项目名（如 `sim-trade`）即可只看该项目（项目名已在 `tokenMatch` 的 haystack 内）。

## 7. L2：同项目联动

### 7.1 数据流
`CodeloopPage` 已同时持有 `claudeId` / `codexId` / `sessions`。为每个 `SideSelect` 传入
**对侧已选会话的项目名**作为亲和项目：
- Codex 选择器收 `affinityProject = projectName(cwd(已选 Claude 会话))`。
- Claude 选择器收 `affinityProject = projectName(cwd(已选 Codex 会话))`。
- 对侧未选时 `affinityProject` 为空 → 退化为纯 L1 行为。

实现：在 `CodeloopPage` 加一个 `cwdOf(id)`/`projectOf(id)` 查表（从 `sessions` 找 `id` → `cwd`
→ `projectName`），把结果作为新 prop 下发。

### 7.2 排序与分组调整（当 `affinityProject` 非空）
- **亲和项目组置顶**，组头加徽标"匹配当前选择"（蓝色弱底），其余项目正常按 6.1 排在其后。
- 在亲和项目组与其余之间插入一条**"其他项目"分隔**（弱色细标题），其下的会话视觉弱化
  （`opacity-70` 或灰色文本），**仍可点击选中**。
- 选中异项目会话**不阻止**——保持现有"点了启动再校验"的兜底；仅靠视觉弱化引导。

### 7.3 可选轻提示（实现时一并做）
- 当对侧已选、且本侧当前选中项与亲和项目不一致时，在选择器下方给一行弱提示：
  "Claude 与 Codex 不在同一项目，启动时会校验失败"。纯提示，不拦截。

## 8. 组件改动清单（全部前端）

| 文件 | 改动 |
|---|---|
| `SessionPairPicker.tsx` | 新增 `relativeTime()`；`SideSelect` 增 `affinityProject?: string`；下拉渲染从扁平列表改为"分组 + 组头 + 相对时间"；亲和项目置顶 + "其他项目"弱化分组 |
| `CodeloopPage.tsx` | 加 `projectOf(id)` 查表；给两个 `SideSelect` 分别下发对侧的 `affinityProject`；（可选）不一致弱提示 |

不改：`tauri-client.ts` 类型、后端、Tauri 命令。

## 9. 边界与处理

- **空 `cwd`**：归入"未知项目"组，排最后。
- **同名不同路径**（不同盘/worktree 但末段同名）：本设计按"末段名"分组，可能把
  `D:\a\sim-trade` 与 `E:\b\sim-trade` 并到一组。当前可接受（展示层）；如需严格区分，后续可在
  组头 `title` 悬浮完整 `cwd`。**L2 亲和判断同样基于末段名**，故上述同名情况会被视为同项目——
  与三方校验（按真实路径）可能不一致，但只影响"是否弱化"，不影响启动正确性（仍以后端校验为准）。
- **会话很多**：下拉已 `max-h-64 overflow-auto`；分组头 sticky 保证滚动时仍可见所属项目。
- **筛选 + 分组**：先筛选再分组，避免显示空组。

## 10. 验收清单

- [ ] 两个下拉均按项目分组，组头带 📁 + 项目名，组内/组间按时间倒序。
- [ ] 行时间为相对时间，hover 显示绝对时间 + AI 标题。
- [ ] 输入关键字筛选后，空项目组隐藏；无结果显示"无匹配会话"。
- [ ] 选定 Claude 会话后，Codex 下拉把同项目组置顶 + "匹配当前选择"徽标，异项目下沉到"其他项目"且弱化。
- [ ] 反向亦然（先选 Codex）。
- [ ] 异项目会话仍可点选；跨项目组合点"启动"时仍由后端三方校验拦截并报错（行为不回归）。
- [ ] `tsc --noEmit` 通过。

## 11. 不在本期（后续可选）

- L3：项目优先选择器（先选项目，再在组内挑 Claude+Codex）。
- 严格按真实 `cwd` 分组/亲和（区分同名不同路径）。
- 相对时间随时间自动刷新（当前仅打开时计算一次）。
