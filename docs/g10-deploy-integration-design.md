# G10 部署接入规范（服务侧改造设计）

> 目标读者：`D:\git` 下各服务型项目（zero / trace-hub / system-prompt-show / alarm-server …）的维护者。
> 本文给出**让一个服务接入 zero-desktop「G10 部署面板」**所需的全部服务侧改造，以 `english`
> 仓的已落地改造为参考实现。照此 checklist 改完，面板即可对该服务做：连通性探测、本地/远端
> 编译版对比、一键交叉编译部署（端口经环境变量注入）。

- **面板代码**：`crates/zero-desktop/src/modules/g10_deploy/`（`mod.rs` 编排 + `registry.rs` 清单）
- **参考实现**：`D:\git\english`（commit 见该仓）—— 本文所有「参考」均指它
- **依赖底座**：`custom-utils ≥ 0.16`（提供 `install -e/--env KEY=VAL` → systemd `Environment=` 注入）

---

## 1. 面板契约（面板期望服务满足什么）

面板对每个服务做四件事，分别有各自的契约：

| 能力 | 面板侧实现 | 对服务的要求 |
|------|-----------|-------------|
| HTTP 健康探测 | `g10_probe_service` 读 `/health` 的 `status`/`version`/`commit` | 服务要有返回这三字段的健康端点 |
| 端口 TCP 探测 | `g10_probe_ports` connect 各登记端口 | 端口在 registry 中登记，且服务确实监听 |
| 本地编译版 | `g10_local_version` 跑 `git -C <repo_dir> rev-parse --short HEAD` + dirty | 本地仓库可达（registry 填对 `repo_dir`） |
| 一键部署 | `g10_deploy` 以仓库根为 cwd 起 `pwsh -File <deploy.script> <args> -Bind 0.0.0.0:<port>` | 仓内有部署脚本，且端口经环境变量注入 |

**关键链路（端口注入）**：面板把该服务 registry 的主端口 `ports[0]` 拼成 `0.0.0.0:<port>`，作为
`-Bind` 参数**自动追加**给部署脚本（脚本已含 `-Bind` 则不重复，见 `mod.rs` 的 deploy_args 逻辑）。
部署脚本据此在安装时把 `<SERVICE>_BIND=<bind>` 写进 systemd unit 的 `Environment=`，使「改端口
无需改代码重编译」。

---

## 2. 服务侧改造 checklist

逐项照做。括号内为 english 参考位置。

### 2.1 依赖升级到 `custom-utils ≥ 0.16`（Cargo.toml）

```toml
custom-utils = { version = "0.16", default-features = false, features = ["updater", "logger"] }
```

0.16 新增 `install` 子命令的可重复 `-e/--env KEY=VAL`，在 `dispatch` 里解析后追加进
`ServiceConfig.env`，渲染为 unit 的 `Environment=` 行（CLI 值在 builder `.env(..)` 之后，
同名 key CLI 覆盖 —— systemd 末行优先）。无此版本则端口无法经命令行注入。

### 2.2 健康端点返回 `{status, version, commit}`（参考 `src/main.rs` `health_check`）

面板 `g10_probe_service` 只认这几个字段：

```rust
async fn health_check() -> Result<HttpResponse> {
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),   // 远端【运行版】= 语义版本
        "commit": env!("GIT_COMMIT_HASH"),       // 远端【编译版】= git 短哈希（见 2.3）
        "timestamp": chrono::Utc::now().to_rfc3339()
    })))
}
```

- 端点路径自定（`/health`、`/api/web/health` 均可），但要与 registry 的 `health_url` 一致。
- 必须返回 **2xx + JSON**；缺字段不报错（面板对应列显示缺失），但 `version`/`commit` 是面板
  对比「本地编译版 vs 远端运行版」的依据，建议都给。

### 2.3 编译期嵌入 git 短哈希（新增 `build.rs`，参考 `english/build.rs`）

```rust
use std::process::Command;
fn main() {
    let hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=GIT_COMMIT_HASH={hash}");
    println!("cargo:rerun-if-changed=.git/HEAD");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD") {
        if let Some(r) = head.strip_prefix("ref:").map(str::trim) {
            println!("cargo:rerun-if-changed=.git/{r}");
        }
    }
}
```

> 交叉编译在 Docker 容器内进行：容器要能看到 `.git`（仓库整体挂载即可，english 的 deploy 脚本
> 挂 `${RepoRoot}:/root/src`）；否则哈希回退 `unknown`（不阻断构建）。

### 2.4 监听地址走环境变量 `<SERVICE>_BIND`（参考 `src/main.rs` bind 段）

约定每个服务用大写服务名前缀 + `_BIND` 作为环境变量，值为**完整 `host:port`**：

| 服务 | 环境变量 | 示例值 |
|------|---------|--------|
| toolkit-server | `TOOLKIT_BIND` | `0.0.0.0:8788` |
| english | `ENGLISH_BIND` | `0.0.0.0:28080` |
| **新服务** | `<UPPER_NAME>_BIND` | `0.0.0.0:<port>` |

解析规则：**环境变量存在即覆盖 CLI `--port`/`--bind`，否则回退默认**。

```rust
let bind_addr = std::env::var("ENGLISH_BIND")
    .ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    .unwrap_or_else(|| format!("0.0.0.0:{}", args.port));
// server.bind(&bind_addr) / .bind_rustls(bind_addr, cfg)
```

### 2.5 systemd unit 的 ExecStart 不再写死端口（参考 `build_svc`）

```rust
LinuxService::new(<unit>, <gh-owner>, <gh-repo>, env!("CARGO_PKG_VERSION"))
    .description(<desc>)
    .exec_args("-w {workspace}")   // ← 去掉 "--port 28080" 之类；端口改由 Environment 提供
    .watchdog_sec(30)
```

端口来自安装时注入的 `Environment=<SERVICE>_BIND=...`（见 2.6），ExecStart 只保留 workspace 等。

### 2.6 部署脚本 `deploy-g10.ps1`（仓库根，参考 `english/deploy-g10.ps1`）

职责：**交叉编译 → scp → `install -e <SERVICE>_BIND=<Bind>` → 重启**。要点：

- `param` 暴露 `-Bind`（默认主端口）、`-Service`、`-Workspace`、`-Features`、`-SkipBuild/-SkipInstall/-SkipRestart`。
  **面板会自动追加 `-Bind 0.0.0.0:<port>`，脚本必须接住这个参数名**。
- 安装步骤用新机制注入端口：
  ```powershell
  $dest install --workspace $Workspace -e <SERVICE>_BIND=$Bind
  ```
- scp 用 `.new` 临时名 + `install -m 755 / mv` 覆盖（避免覆盖运行中二进制的 ETXTBSY）。
- 非交互 ssh 跑 `systemctl --user` 前先 `export XDG_RUNTIME_DIR=/run/user/$(id -u)`。
- 交叉编译镜像/挂载沿用各仓既有方式（english 用 `huangjiemin/rust_aarch64-gcc_openssl:…`，
  `SQLX_OFFLINE=true`，独立 `target-aarch64` 目录）。

> 已有 `deploy-g10.ps1` 的仓（如 toolkit-server）只需补「install 注入 `<SERVICE>_BIND`」这步，
> 其余链路不变。

---

## 3. zero-desktop 侧接入（registry.rs）

在 `crates/zero-desktop/src/modules/g10_deploy/registry.rs` 的 `builtin()` 里把目标服务的
`deploy` 由 `None` 改为 `Some(DeployDef{..})`（english 已示范）：

```rust
ServiceDef {
    name: "<name>".into(),
    label: "<显示名>".into(),
    note: "<一句话>".into(),
    repo_dir: r"D:\git\<repo>".into(),         // g10_local_version 跑 git 的目录
    health_url: "http://192.168.0.68:<port>/health".into(),  // 与 2.2 端点一致
    remote_service: Some("<name>.service".into()),
    web_url: String::new(),
    host: default_host(),
    ports: vec![PortInfo { port: <port>, note: "<用途>".into() }],  // ports[0] = 主端口，面板据此拼 -Bind
    deploy: Some(DeployDef {
        script: "deploy-g10.ps1".into(),       // 相对 repo_dir
        args: vec!["-Service".into(), "<name>".into()],  // -Bind 由面板自动追加，勿在此重复
    }),
},
```

> registry 支持 workspace 下 `g10-services.json` 覆盖文件，可不改代码、先用覆盖文件验证，
> 稳定后再回写 `builtin()`。

---

## 4. 数据流（一键部署时序）

```
面板「部署」按钮
  └─ g10_deploy: cwd=repo_dir 起 pwsh -File deploy-g10.ps1 -Service <name> -Bind 0.0.0.0:<port>
       └─ deploy-g10.ps1:
            1) docker 交叉编译 aarch64 release
            2) scp 二进制 → ~/.local/bin/<name>.new → install -m 755 覆盖
            3) <name> install --workspace <ws> -e <SERVICE>_BIND=<Bind>
                 → 写 ~/.config/systemd/user/<name>.service 的 Environment=<SERVICE>_BIND=<Bind>
                 → daemon-reload
            4) systemctl --user restart <name>
  └─ 服务启动: 读 <SERVICE>_BIND 环境变量 → bind 该地址
  └─ 面板探测: GET <health_url> → {status, version, commit} → 绿灯 + 版本对比
```

---

## 5. 命名与约定速查

| 项 | 约定 | 示例 |
|----|------|------|
| 环境变量 | `<UPPER_SERVICE>_BIND`，完整 `host:port` | `TRACE_HUB_BIND=0.0.0.0:9100` |
| systemd unit / service | `<name>.service` | `trace-hub.service` |
| 部署脚本 | 仓库根 `deploy-g10.ps1` | — |
| 健康端点 | 返回 `status/version/commit` 的 2xx JSON | `/health` |
| 远端二进制目录 | `~/.local/bin/`（custom-utils updater 默认） | — |
| 远端 workspace | `~/.config/<name>/` | `~/.config/trace-hub/` |
| 优先级 | env `<SERVICE>_BIND` > CLI `--port`/`--bind` > 默认 | — |

---

## 6. 注意事项 / 各仓差异

- **HTTP vs HTTPS 探测**：面板 `g10_probe_service` 用普通 `reqwest::Client`，**不接受自签证书**。
  若服务跑 HTTPS（如 english 的 `prod` feature）且 registry `health_url` 写 `http://`，探测会失败
  （红灯不影响部署本身）。三选一：① 健康端点单独走 HTTP；② registry 用 `https://` 并给探测
  client 开 `danger_accept_invalid_certs`（改 `mod.rs`）；③ 接受红灯。**接入前先定本服务的探测协议**。
- **virtual workspace 仓**（多 crate，如 toolkit-server）：交叉编译需 `-p <crate>`，且本地 path
  依赖（如 `custom-utils = { path = "../custom-utils" }`）要把同级目录一并挂进容器。单 crate 仓
  （如 english）直接 `cargo build --bin <name>` 即可。
- **C 交叉依赖**（openssl / ring / aws-lc-sys）：用预置交叉镜像，必要时补 `AR_/CC_/CXX_/LINKER_`
  环境变量（english/ toolkit 的脚本已示范）。纯 rustls 栈（english）比 openssl 省心。
- **前端 embed**：带 web 前端的服务（english `static`/`prod` feature）交叉编译前需先产出 `frontend/dist/`
  （rust-embed 编译期读取）。
- **install 幂等**：`<name> install` 重写 unit + daemon-reload，可反复跑；改端口=带新 `-Bind`
  重跑 install（或整条 deploy）即可，无需重编译。

---

## 7. 验收 checklist

- [ ] `Cargo.toml` custom-utils ≥ 0.16，`cargo check` 通过
- [ ] 新增 `build.rs`，`/health` 返回非空 `version` + `commit`
- [ ] 监听地址读 `<SERVICE>_BIND`，env 覆盖 CLI；`build_svc().exec_args` 无写死端口
- [ ] `<name> install --dry-run -e <SERVICE>_BIND=0.0.0.0:<port>` 预览到 `Environment=<SERVICE>_BIND=...`
- [ ] 仓库根有 `deploy-g10.ps1`，接收 `-Bind`，install 步骤注入 `<SERVICE>_BIND`
- [ ] zero-desktop `registry.rs` 该服务 `deploy: Some(..)`，`health_url`/`ports`/`repo_dir` 填对
- [ ] 面板：连通性绿灯、版本/commit 显示、一键部署跑通、改端口生效
```
