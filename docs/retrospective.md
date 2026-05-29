# 抖音 Skill v1 复盘与重构动议

> 状态：v1 已交付（2026-05-28 ~ 30），存在结构性能力局限。本文档固化 v1 实施过程的现状 / 问题 / 已知根因 / 已试方案 / 未试方向，作为 v2 设计的输入。
> 用户提出"我要重构这块功能"（2026-05-30）后开此文档，按 `memory/feedback_preserve_design_rationale.md` 要求趁热完整留档。

## 时间线

| 日期 | 事件 |
|---|---|
| 2026-05-28 | v1 设计评审 + ADR `2026-05-28-douyin-skill.md` 落档；fork rebase 到 jiji262 上游 main；zero_tool 子包 + 5 工具 + douyin skill + 主 prompt 触发段一次性交付；g10 部署完毕 |
| 2026-05-29 | 首次端到端测试发现主 Claw 绕过 OrchestrateTask 直调工具——SKILL.md prompt 强化「抖音相关一律委派」；加 douyin_set_cookie 工具 + douyin-cookie-update skill；加 douyin_search_user 工具 + 修 shadow-throttle 退出逻辑；SKILL.md 改造（标准执行序 → 能力清单 + 业务铁律） |
| 2026-05-30 | 实测：博主 aweme_count=80 但 list_works 只拿 15 条；diff 浏览器真实请求 vs jiji262 请求，发现 from_user_page / uifid / verifyFp / webid 等多项缺失；补 4 个参数 + 动态 Referer 后**完全无效**（cursor 序列 1776914346000 → 1775898987000 → 1774693906000 → 1772297669000 → has_more=False，与修复前一模一样） |

## 1. v1 能力盘点

### 1.1 工具清单（共 7 个 CLI 子命令 / 7 个 nova 工具）

| 工具 | Python 文件 | 作用 |
|---|---|---|
| `douyin_cookie_status` | `zero_tool/cookie_status.py` | cookie 自检（调 `api_client.get_self_info`） |
| `douyin_set_cookie` | `zero_tool/set_cookie.py` | 写入 cookies.json，自动识别浏览器 Cookie 头 / JSON 对象 |
| `douyin_search_user` | `zero_tool/search_user.py` | 按昵称 / 抖音号搜博主（调 `api_client.search_user`） |
| `douyin_resolve_user` | `zero_tool/resolve_user.py` | URL/短链/sec_uid → 博主完整资料（aweme_count 等） |
| `douyin_list_works` | `zero_tool/list_works.py` | 取作品元数据（含 throttled / pages_fetched 信号） |
| `douyin_download_submit` | `zero_tool/download_submit.py` | 异步入队下载，立即返 task_id |
| `douyin_download_status` | `zero_tool/download_status.py` | 查任务进度 |

### 1.2 Skill 清单（2 个）

- `.zero/skills/douyin/` — 主下载 skill，preload 6 个工具（不含 set_cookie）
- `.zero/skills/douyin-cookie-update/` — cookie 更新 skill，preload `douyin_set_cookie`

### 1.3 依赖与部署

- douyin-downloader fork：`jm-observer/douyin-downloader` HEAD `4eda82c`（领先上游 `jiji262:main` 4 个 commit）
  - `e35ad47` feat: 新增 search_user
  - `7804051` fix: search_user 补 search_filter_value 等参数
  - `dba4f07` feat: search_user 检测 verify_check 返 anti_bot
  - `cb0fd1e` fix: list_works shadow-throttle 退出逻辑 + throttled 信号
  - `4eda82c` fix: get_user_post 加 from_user_page/uifid/verifyFp/fp + 动态 Referer + need_time_list=0
- archive 备份分支 `archive/pre-upstream-sync-2026-05-28`（已 push 留底）
- g10 部署：`~/douyin-downloader/venv/` Python 3.12 + `~/.config/zero/{skills,tools.d,douyin/cookies.json}`
- systemd drop-in `~/.config/systemd/user/zero.service.d/z-douyin.conf`：PATH 含 venv/bin、PYTHONPATH=`~/douyin-downloader`

### 1.4 实施工程量

- Python 新增 ~700 行（zero_tool/）+ ~60 行（api_client.py 加 search_user / 改 _request_json 等）
- Rust 改动：**0 行**（按设计零 Rust 改动达成）
- pytest：34 个用例全绿（本地 + g10 双验证）
- 文档 6 份：ADR + 总览 + Plan 1/2/3 + cookie-setup
- prompt：zero.md 加 2 段委派（抖音内容下载 + 抖音 Cookie 更新），SKILL.md 两个 skill

## 2. 实际能用的部分（✅）

| 能力 | 验证证据 |
|---|---|
| 查博主基本信息 + 作品总数（`aweme_count`） | 2026-05-29 23:57 sessions.db msg 2563-2565：用户给 H5 share URL，子 Agent 调 resolve_user 一次拿到 `aweme_count:80`，**未调 list_works** 直接报 "80 个"。SKILL.md 改造后的标杆 case |
| 写入 cookie | set_cookie 实测 41 字段落盘，has_required=[msToken,ttwid,sessionid_ss] |
| 主 Claw → OrchestrateTask 委派 | 2026-05-29 prompt 强化后，新 session 测试主 Claw 不再绕过；铁律「禁止主 Agent 直调」落实 |
| 异步下载任务模型 | download_submit 立即返 task_id；状态文件原子替换；download_status 轮询；尚未端到端实测真实下载 |
| 单工具失败错误码（cookie_missing / anti_bot / invalid_input / network_error / not_found / api_failure / internal_error）+ 子 Agent 诚实汇报 | SKILL.md §2.1「报数字一定要诚实」、§2.3 错误对照表；2026-05-29 23:59 子 Agent 汇报"共 80 个，本次拉 15 条（限流）"——避免误说"博主只有 15 条" |

## 3. 实际不能用 / 受限的部分（❌）

### 3.1 用户搜索（按昵称 / 抖音号查 sec_uid）

**症状**：`search_user` / `search_aweme` 均触发 `search_nil_info.search_nil_type=verify_check`，搜索结果一律为空 list。

**已确认**：
- 不是参数缺失（已加齐 cv-cat 用过的 `search_filter_value` / `is_filter_search=1` / `need_filter_settings`）
- 不是端点差异（试过 `/general/search/single/` 与 `/discover/search/`，结果相同）
- 整个搜索接口集群被风控锁

**已实施兜底**：search_user 检测 `verify_check` 后返回 `error=anti_bot`，SKILL.md 教 LLM 转告用户「请直接发主页 URL，跳过搜索」。

### 3.2 列博主完整作品

**症状**：博主 `aweme_count=80`，但 `get_user_post` 4 页就 `has_more=False`，cursor 截断在第 15 条。

**实测 cursor 序列（多次重复完全一致）**：
```
p1: items=1  has_more=True  cursor=1776914346000
p2: items=0  has_more=True  cursor=1775898987000
p3: items=5  has_more=True  cursor=1774693906000
p4: items=9  has_more=False cursor=1772297669000 ← 抖音明确告知"没了"
```

**已确认不是因子（验证后 cursor 序列不变）**：
- ❌ 加 `from_user_page=1`
- ❌ 加真 `uifid`（256 hex，从 cookie UIFID 抽）
- ❌ 加 `verifyFp` / `fp`（从 cookie `s_v_web_id` 抽）
- ❌ Referer 动态改为博主主页 URL
- ❌ `need_time_list=0` 对齐浏览器
- ❌ MAX_PAGES 限制（实际只翻了 4 页，没碰我们的 30 上限）
- ❌ 调高 PAGE_SIZE（20 vs 浏览器 18，无实质差异）

**仍未尝试 / 怀疑的因子**：
- `webid=7645128523896686132` — URL query 参数，**不是 cookie**，浏览器 JS 运行时计算
- `x-secsdk-web-signature` — 另一层 SDK 签名（与 a-bogus 不同）
- `timestamp` — 浏览器每次重算，jiji262 未传
- a-bogus 算法版本鲜度 — jiji262 内置的 `utils/abogus.py` 实现可能落后于抖音最新算法
- IP 指纹 — cookie 可能 bind 到首次登录的设备/IP，g10 用同 cookie 走另一 IP 触发降级

**已实施兜底**：list_works 返回 `throttled / pages_fetched` 信号；SKILL.md §2.1 教 LLM 用 `resolve_user.aweme_count` 对照 `list_works.count`，诚实汇报"共 M 条，本次拉到 N 条（限流）"而非"博主只有 N 条"。

### 3.3 单视频独立下载

`resolve_user` 只认 user 类型 URL，单视频链接 `v.douyin.com/<aweme_id>` 被拒。v1 范围内不实现，子 Agent 兜底告知用户。

### 3.4 完整下载链路（download_submit + status）

**未实测**——本期受 list_works 截断阻塞，未走到 download。理论上 `get_video_detail`（VideoDownloader 内部用）走的是另一个 endpoint，**未必受 list_works 同样的降级影响**，下载已知 aweme_id 可能可行。**v2 必须先验证**。

### 3.5 cookies.json 字段不完整

实测 g10 上 41 字段，对比浏览器 PowerShell 抓包**缺**：

- `UIFID`、`s_v_web_id`（关键指纹，2026-05-30 临时手工 patch 进去过，无效）
- `xgplayer_device_id`、`xgplayer_user_id`、`dy_swidth`、`dy_sheight`
- `fpk1`、`fpk2`
- `__ac_nonce`、`__ac_signature`
- 其他 `IsDouyinActive`、`stream_recommend_feed_params` 等

**根因猜测**：用户从浏览器 DevTools Application 面板复制 cookie 时，只展开了 `.douyin.com` 域，未跨展 `www.douyin.com` 子域。`set_cookie` 工具本身无 bug，但**缺一道"复制全否则可能限流"的用户提示**。

## 4. 关键技术发现

### 4.1 抖音搜索类接口对 cookie / IP 信誉极敏感

同一 cookie 在浏览器能搜，在 g10 不行——`s_v_web_id` 同名同值，但走 zero_tool 时 100% verify_check。说明抖音判定不只看 cookie，还看其它请求质量信号。

### 4.2 list_works 看似"shadow throttle"，实际是抖音确定性截断

误诊路径：
1. 第一次见 1 条就 break（旧 list_works "saw_any + 单空页 → 退出"逻辑） → 误认为"博主只有 1 条作品"
2. 修退出逻辑（连续 5 空页才退）后拿到 15 条 → 误以为是"平均 0.5 条/页 × MAX_PAGES=30 = 15"的限流
3. 实测翻页（直接调 api_client.get_user_post）后看到只跑了 4 页就 `has_more=False` → 才意识到是抖音明确截断

教训：**先实测真实 API 响应，再写代码**。避免基于猜测的层层补丁。

### 4.3 浏览器请求 vs jiji262 请求的关键 diff（2026-05-30 PowerShell 抓包）

| 项 | 浏览器 | jiji262 现状 | 验证是否因子 |
|---|---|---|---|
| `from_user_page` | `1` | 缺失 | ❌ 加上后无变化 |
| `webid` | `7645128523896686132` | 缺失（不在 cookie） | ⚠️ 未验证（需逆向 JS 算法） |
| `uifid` | 256 hex 真值 | 空字符串 | ❌ 改真值后无变化 |
| `verifyFp` / `fp` | `verify_mpq9qiww_...`（= cookie `s_v_web_id`） | 缺失 | ❌ 加上后无变化 |
| `x-secsdk-web-signature` | 有 | 缺失 | ⚠️ 未验证 |
| `timestamp` | epoch | 缺失 | ⚠️ 未验证 |
| Referer | 博主主页 URL | 写死 `?recommend=1` | ❌ 改动态后无变化 |
| `need_time_list` | `0` | `1` | ❌ 改 0 后无变化 |
| `count` | 18 | 20 | ❌（试过 20 / 18 都一样） |

**剩余强嫌疑**：`webid` + `x-secsdk-web-signature` + a-bogus 鲜度三个组合，或 IP 层面。

### 4.4 a-bogus 与 X-Bogus

jiji262 已优先用 a-bogus（`utils/abogus.py`），fallback X-Bogus。但实测搜索接口仍被 verify_check。说明**a-bogus 算法本身可能版本落后**于抖音当前线上算法，或抖音对低质量 a-bogus 直接返回降级数据。

cv-cat 的 a-bogus 也是同 endpoint 用，未必比 jiji262 新。但 cv-cat 多了 `web_id` 注入（`with_web_id(auth, refer)`），值得逆向看。

## 5. v1 设计上做对的事（v2 应保留）

| 决策 | 复盘 |
|---|---|
| 零 Rust 改动 + 纯 skill 委派 | ✅ 完全融入 zero 既有 nova-agent 体系，6 工具 + 2 skill 没碰 Rust，rebase 友好 |
| 异步入队下载（submit / status 拆分） | ✅ 完全绕开 nova 30s 硬超时，工业级长任务模型，v2 保留 |
| SKILL.md「能力清单 + 业务铁律」模式 | ✅ 2026-05-30 测试用 aweme_count 直答 vs 80 验证了 LLM-first 设计能 work，避免了"标准执行序"的弱模型代偿 |
| 诚实汇报机制（throttled / aweme_count 对照 / 错误码模板） | ✅ 即使工具拿不全，子 Agent 不会瞎编"博主只有 N 条"，体验上没事故 |
| fork 同步上游 + 自己加 zero_tool 子包（独立目录） | ✅ rebase 时只在 zero_tool/ 有冲突风险，上游 core/utils/auth/* 0 修改 |
| archive 备份分支 | ✅ force push 前留底，可回滚 |
| 工具粒度（7 个细粒度而非 1 个聚合） | ✅ LLM 看着元数据决策，可观测、可重试、可分支 |

## 6. v1 设计上做错 / 估算错的事（v2 应避免）

| 决策 / 行为 | 复盘 |
|---|---|
| 低估抖音风控对 web API 的限制强度 | 以为"通用搜索 + a-bogus + 登录 cookie"够用，实际**完整指纹 + 真实浏览器请求质量评分**才是抖音的判定基线 |
| 过早实施 search_user | 上线即被 verify_check 100% 拦截。本应**先验证既有 search_aweme 在 cookie / IP 下可用**再扩。结果 search_user 现在是"装饰性工具"，全靠 anti_bot 兜底 |
| SKILL.md 初版「标准执行序」 | 死板的 1-6 流程被用户当场指出"为什么要定这个标准"。**架构上 LLM-first 应当先期就避免**这种弱模型代偿 |
| list_works 退出逻辑两次纠错 | 第一次单空页就退、第二次连续 5 空页才退——都基于猜想。**应当先实测 cursor 序列 + has_more 行为再写代码** |
| 修 list_works 时引入"throttled 检测"基于"平均 < 2 条/页" | 2026-05-30 实测 15 条 / 4 页 = avg 3.75 > 2，throttled=false，**误导信号**。实际是抖音 has_more=False 主动截断，与 throttle 阈值无关。判定应改为 `has_more=False AND count << aweme_count` |
| 加 from_user_page / 真 uifid / Referer 等修复**没先实验验证就 commit** | `4eda82c` 这个 commit 内容是"修复"但实测无效。**应当先小步实验确认有效再 commit**，否则 fork 上堆一堆"看起来对"但其实没用的改动 |

## 7. v2 重构方向（待用户决策）

### 方向 A：替换 HTTP 客户端实现，深度模拟真实浏览器

- 留 zero_tool / skill / nova-agent 集成层不变
- 替换 douyin-downloader 的 HTTP 层为完整指纹版本：
  - **A1**：基于 cv-cat 的请求构造（`with_web_id` / 真 a-bogus chain + 完整 cookie 子域），把它移植到 jiji262 async 上下文
  - **A2**：自己写 HTTP 客户端，逆向当前浏览器 JS（webid / x-secsdk-web-signature 生成算法 / a-bogus 最新版）
- **代价**：开发 + 持续对抗维护（抖音每月可能改一次）
- **收益**：跨 fork 框架自由度
- **风险**：抖音改一次，A 路径全失效，重做

### 方向 B：用 Playwright 浏览器自动化（headless Chromium）

- zero_tool 调 Playwright 进程加载真实博主主页 → DOM 滚动触发懒加载 → 真实浏览器发请求 → 抓 aweme_id 列表 / 视频 URL
- 协议层 100% 绕开签名 / 指纹（浏览器自带）
- **代价**：
  - Playwright 进程沉（200-400MB / 实例）
  - g10 上 headless Chromium 要装一堆系统 lib（libnss3 / libgbm1 等）
  - 抖音 `nav.webdriver` 反自动化检测，要做 stealth
  - 首次扫码登录人工
- **收益**：抖音改签名 / 反爬不影响（浏览器层级稳定）
- **风险**：Playwright stealth 也是对抗游戏，但比 A 路径变化频次低

### 方向 C：保留 v1，接受已知限制

- 不重构，把 v1 当前能力作为正式交付
- 文档明确写明限制：「v1 仅支持博主前 ~15 条最近作品 + 看 aweme_count + 下载已知 aweme_id」
- 用户使用模式：**浏览器找 aweme_id → 喂给 zero 走 download_submit**（这条路径还没实测，**v2 前置验证项**）
- **代价**：体验差，用户每次要手动拿 aweme_id
- **收益**：0 工程

### 方向 D：换数据源 / 第三方反代

- 不爬抖音 web，用：
  - 抖音开放 API（要资质申请）
  - 第三方反代服务（如某些聚合 API）
  - 抖音 PC 客户端协议（mDNS / WebSocket，逆向工程）
- **代价**：依赖第三方稳定性 / 合规风险
- **收益**：跳出 web 反爬体系

### 方向 E：混合（推荐评估）

- **看博主资料 + 作品总数** → 走 web API（A，当前已稳）
- **列博主完整作品** → 浏览器自动化（B）
- **下载视频本体** → 走 web API（A，未实测但可能稳）
- **写入 cookie** → 走 web API（当前已稳）
- **搜博主** → 浏览器自动化（B，绕开 verify_check）或暂时不做

E 优势：把"用 web API 能搞定的"留 web API（低成本），把"用 web API 搞不定的"切浏览器（一致性）。

## 8. 待重构前要回答的问题

按重要性排：

1. **真实业务场景频次**——你下载抖音的真实需求是：
   - (a) 偶尔下载几个视频（v1 当前能力已基本够）
   - (b) 备份某博主全量作品（v1 做不到，需 v2）
   - (c) 监听博主更新自动下新作品（v1 做不到）
2. **能接受的手工程度**——
   - (a) 自动化：贴个博主链接，agent 自动拉全部
   - (b) 半自动：浏览器找 aweme_id 列表给 agent，agent 批量下
   - (c) 全手动：每个视频单独下
3. **g10 能否上 Playwright**——GB10 设备资源充足（aarch64 + CUDA13 + 16GB+），Playwright headless Chromium 完全跑得动；问的是**你愿不愿意接受多一个常驻进程的运维**
4. **维护对抗预算**——抖音反爬是持续战（一年可能改 2-4 次关键签名 / 反爬规则）。谁来追？预算多少时间？
5. **是否要修复 search_user**——当前 anti_bot 兜底已经"够用"（让用户给 URL），是否要花精力让它真的能搜？

## 9. v2 启动前必做的 5 分钟验证

在决定方向前，**先实测**这两件事，避免 v2 走错路：

### 9.1 download_submit + download_status 完整链路

用 list_works 拿到的某个 aweme_id（如 `7632929798888656155`），跑：

```bash
ssh fengqi@192.168.0.68 'env -i HOME=$HOME PATH=/home/fengqi/douyin-downloader/venv/bin:/usr/bin PYTHONPATH=/home/fengqi/douyin-downloader python -m zero_tool download_submit --ids 7632929798888656155'
# 拿到 task_id 后
ssh fengqi@192.168.0.68 'env -i HOME=$HOME PATH=/home/fengqi/douyin-downloader/venv/bin:/usr/bin PYTHONPATH=/home/fengqi/douyin-downloader python -m zero_tool download_status --task-id <task_id>'
# 等几次直到 state=succeeded 或 failed
ls ~/.config/zero/downloads/douyin/7632929798888656155/
```

**关键问题**：`get_video_detail` 是否也受 `get_user_post` 同样的降级影响？如果**下载本体能成功**，那 v1 的 hybrid 兜底（用户手动给 aweme_id）就是真可用的，v2 优先级降一档。

### 9.2 用浏览器 cookie 整套（含 UIFID + s_v_web_id 等所有 64+ 字段）重新 set_cookie 后，list_works 是否能多拿一些

走 douyin-cookie-update skill，把 PowerShell 抓包脚本里**所有** cookie 字段拼成 Cookie 头格式更新进去。然后跑 `python -m zero_tool list_works --sec-uid <...> --limit 80`，看 count 是否 > 15。

如果**还是 15**：cookie 完整度不是因子，根因在 webid / 签名 / IP 层。
如果**变多了**：缺字段是关键因子，v2 至少要先把 set_cookie 工具改成"拒绝接受不完整 cookie"。

## 10. 关联文档与代码

### v1 文档
- ADR：[`docs/adr/2026-05-28-douyin-skill.md`](../adr/2026-05-28-douyin-skill.md)
- 任务总览：[`douyin-skill.md`](./douyin-skill.md)
- Plan 1（Python 子包）：[`douyin-skill-plan-1.md`](./douyin-skill-plan-1.md)
- Plan 2（nova 工具 + skill）：[`douyin-skill-plan-2.md`](./douyin-skill-plan-2.md)
- Plan 3（主 prompt + 端到端）：[`douyin-skill-plan-3.md`](./douyin-skill-plan-3.md)
- Cookie 部署文档：[`cookie-setup.md`](./cookie-setup.md)
- 本文档：[`retrospective.md`](./retrospective.md)

### 关键代码
- zero 仓 `d8357a3` `feat(douyin)` + `a265e22` `fix(prompts)` — v1 主体
- douyin-downloader fork main HEAD `4eda82c`
- `core/api_client.py:313-345` `_build_user_page_params`（含本期加的 from_user_page / uifid / verifyFp / fp）
- `core/api_client.py:388-410` `get_user_post`（含本期加的动态 Referer、need_time_list=0）
- `core/api_client.py:495-503` `get_user_info`（拿 aweme_count 的接口，**当前能用**）
- `zero_tool/list_works.py:_list`（含 shadow-throttle 退出逻辑 + throttled 信号）

### memory 引用

- `feedback_preserve_design_rationale.md`：本文档存在的理由
- `feedback_minimal_diff_preference.md`：方向选择按"最合适"排（v2 不要因贪图最小改动选 C）
- `feedback_no_weak_model_architecture_compromise.md`：SKILL.md 改造的根据
- `project_zero_nova_custom_utils_coupling.md`：v2 如要扩 nova 工具能力需走 tag 流程

---

## 11. v2 Phase 0 验证结论（2026-05-30）

参照 cv-cat `DouYin_Spider` + jiji262 `douyin-downloader` 重写 zero 工具，落地语言定为 **Rust 新 crate `crates/douyin`**（单二进制 + 7 子命令）。Phase 0 用自包含 Python harness（jiji262 纯 Python `abogus.py` + cv-cat 参数配方 + 真实 cookie）在**本机 Windows 与 g10 各跑一遍**，隔离变量。

### 11.1 §4.4 悬案坐实：截断真因 = 出口 IP

同脚本同 cookie，唯一变量是机器/出口 IP。每页 `max_cursor` **逐页完全相同**（`1777021556000→1776072324000→1775117810000→1773092994000→1772297669000`），但每页**条数不同**：

| 机器 | 出口 IP | 归属 | 每页条数 | 总计 |
|---|---|---|---|---|
| 本机 Windows | `38.209.122.38` | 洛杉矶 / Cogent（美国 VPN） | `18,18,18,18,8` | **80/80 ✅** |
| g10 | `58.23.139.139` | 厦门 / 中国联通家宽 | `1,0,1,6,7` | **15/80 ❌** |

- **服务端按出口 IP 信誉抽稀页面**（同游标骨架、抽掉 items），非签名/参数/webid 问题。§3.2「已确认不是因子」清单里的所有改动确实都无效——因为根因压根不在那。
- 反直觉：**美国 VPN IP 放全量、中国家宽 IP 被抽稀**（该 CN 网段疑因抓取历史坏信誉）。
- `webid` 抓取两次都失败（None）仍拿全 80 → **真实 webid 不是关键因子**，推翻 §4.3 的强嫌疑。

### 11.2 各 API 实测判定

| API | 本机 | g10 | 判定 |
|---|---|---|---|
| `user/profile/other`（aweme_count） | 80 | 80 | ✅ 到处可用 |
| `aweme/post`（列作品） | 80/80 | 15/80 | ⚠️ 受出口 IP 抽稀 |
| `aweme/detail`（play_addr+bit_rate 无水印 URL） | ✅ | ✅ | ✅ **g10 也能拿下载 URL** |
| `discover/search`（用户搜索） | verify_check | verify_check | ❌ 美国 IP 也拦死，搜索集群独立锁，与 IP 无关 |

### 11.3 对 v2 的结论

1. **代码路线已验证正确**：a-bogus 用 jiji262 纯 Python 版（f2 ABogus 1.0.1.19，SM3+RC4+自定义 base64，无 Node）做黄金对照逐位移植到 Rust；参数配方见 memory `project_douyin_v2_recipe`。
2. **g10 列全量作品被出口 IP 阻塞** → Phase 4 前置：列作品请求必须走干净代理/VPN（reqwest 客户端需支持可配置 proxy），否则部署后仍 15/80。**待用户决策出口 IP 方案**。
3. profile / detail / **download 在 g10 原生可用** → v1 hybrid 兜底（用户给 aweme_id → 下载）在 g10 真可行。
4. 搜索保留 anti_bot 兜底（让用户给主页 URL）。

验证脚本与原始输出留底：`D:\git\_study\validate\`（仓外，不入库）。
