#requires -Version 7
<#
.SYNOPSIS
    交叉编译 zero 工具集（aarch64-linux）并部署到 G10 设备。

.DESCRIPTION
    Windows 开发机 → 用预置交叉编译镜像在 Docker 容器内构建 aarch64-unknown-linux-gnu
    的 release 二进制（带 prod feature，日志落文件、stdout 保持 JSON 干净）→ scp 到 G10。

    镜像 huangjiemin/rust_aarch64-gcc_openssl 已预置：aarch64 gcc/ar/g++、cmake、clang，
    以及 CARGO_TARGET_*_LINKER / CC_/CXX_ 环境变量（aws-lc-sys 的 C 交叉所需）。

.PARAMETER G10Host
    G10 的 ssh 目标，默认 fengqi@192.168.0.68。

.PARAMETER DestDir
    G10 上的安装目录，默认 ~/.local/bin（与 custom-utils updater 自更新目标一致）。

.PARAMETER SkipBuild
    跳过交叉编译，直接复制已有产物（调试部署用）。

.PARAMETER Service
    部署后要重启的 systemd 用户服务名，默认 toolkit-server（$Bins 里唯一的 daemon；
    其余是 CLI 工具，无需重启）。

.PARAMETER SkipRestart
    跳过部署后重启（仅换二进制，下次服务自然重启时生效）。

.EXAMPLE
    pwsh ./deploy-g10.ps1
    pwsh ./deploy-g10.ps1 -SkipBuild
    pwsh ./deploy-g10.ps1 -SkipRestart
#>
param(
    [string]$G10Host = "fengqi@192.168.0.68",
    [string]$DestDir = "~/.local/bin",
    [string]$Service = "toolkit-server",
    [switch]$SkipBuild,
    [switch]$SkipRestart
)

$ErrorActionPreference = "Stop"
$RepoRoot = $PSScriptRoot
$Target = "aarch64-unknown-linux-gnu"
$Image = "huangjiemin/rust_aarch64-gcc_openssl:1.94.0_9.4.0_1.1.0l_llvm12.0.1"

# (crate package 名, 产物二进制名) —— 新增工具时在此追加一行即可。
$Bins = @(
    @{ Crate = "github_commit_info"; Bin = "github-commit-info" },
    @{ Crate = "hf_watcher";         Bin = "hf-watcher" },
    @{ Crate = "douyin";             Bin = "douyin" },
    @{ Crate = "toolkit-server";     Bin = "toolkit-server" }
)

if (-not $SkipBuild) {
    Write-Host "==> 交叉编译 $Target（Docker: $Image）" -ForegroundColor Cyan
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "未找到 docker，请先安装/启动 Docker Desktop。"
    }

    # 逐 crate 带 prod feature 构建（virtual workspace 必须 -p）。
    $buildCmd = ($Bins | ForEach-Object {
        "cargo build --release --target $Target -p $($_.Crate) --features prod"
    }) -join " && "

    # workspace Cargo.toml 当前用本地 path 依赖 `custom-utils = { path = "../custom-utils" }`，
    # 容器内仓库挂在 /work，故 ../custom-utils 解析为 /custom-utils —— 必须把同级目录也挂进去。
    $CustomUtils = Join-Path (Split-Path $RepoRoot -Parent) "custom-utils"
    if (-not (Test-Path (Join-Path $CustomUtils "Cargo.toml"))) {
        throw "未找到本地 custom-utils（$CustomUtils）；workspace 依赖 path = ../custom-utils，无它无法交叉编译。"
    }

    # 挂载仓库 + 同级 custom-utils；用命名卷缓存 cargo registry，加速重复构建。
    # AR_ 显式补上（镜像只预置了 CC_/CXX_/LINKER）。
    docker run --rm `
        -v "${RepoRoot}:/work" `
        -v "${CustomUtils}:/custom-utils" `
        -v "zero-tools-cargo-registry:/usr/local/cargo/registry" `
        -w /work `
        -e AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar `
        $Image bash -lc $buildCmd
    if ($LASTEXITCODE -ne 0) { throw "交叉编译失败（exit $LASTEXITCODE）" }
}

# 校验产物存在。
$ReleaseDir = Join-Path $RepoRoot "target/$Target/release"
foreach ($b in $Bins) {
    $p = Join-Path $ReleaseDir $b.Bin
    if (-not (Test-Path $p)) { throw "产物缺失：$p（先去掉 -SkipBuild 完整构建）" }
}

Write-Host "==> 部署到 ${G10Host}:${DestDir}" -ForegroundColor Cyan
# 确保远端目录存在。
ssh $G10Host "mkdir -p $DestDir"
if ($LASTEXITCODE -ne 0) { throw "无法在 G10 创建目录 $DestDir（检查 ssh 连通性）" }

foreach ($b in $Bins) {
    $local = Join-Path $ReleaseDir $b.Bin
    $dest = "$DestDir/$($b.Bin)"
    Write-Host "    scp $($b.Bin)"
    # 先传到 .new 临时名，再 mv 覆盖：rename 即使旧二进制正在运行也能替换
    # （直接 scp 覆盖运行中的二进制会 ETXTBSY / dest open Failure）。
    scp $local "${G10Host}:${dest}.new"
    if ($LASTEXITCODE -ne 0) { throw "scp $($b.Bin) 失败" }
    ssh $G10Host "chmod +x ${dest}.new && mv -f ${dest}.new ${dest}"
    if ($LASTEXITCODE -ne 0) { throw "替换 $($b.Bin) 失败（mv）" }
}

# 打印版本确认。
Write-Host "==> 远端版本确认" -ForegroundColor Cyan
foreach ($b in $Bins) {
    ssh $G10Host "$DestDir/$($b.Bin) --version"
}

# 重启 daemon 用户服务（CLI 工具无服务、不涉及；换二进制后服务需重启才加载新版）。
# XDG_RUNTIME_DIR 显式补上：非交互 ssh 默认不带，systemctl --user 会找不到 user bus。
if (-not $SkipRestart) {
    Write-Host "==> 重启 $Service" -ForegroundColor Cyan
    $restartCmd = 'export XDG_RUNTIME_DIR=/run/user/$(id -u); ' `
        + "systemctl --user restart $Service && " `
        + "sleep 2 && " `
        + "systemctl --user is-active $Service && " `
        + "systemctl --user status $Service --no-pager -n 5"
    ssh $G10Host $restartCmd
    if ($LASTEXITCODE -ne 0) { throw "重启 $Service 失败（检查服务名 / 是否已 systemctl --user enable）" }
} else {
    Write-Host "==> 跳过重启（-SkipRestart）" -ForegroundColor DarkGray
}

Write-Host "==> 完成。若 $DestDir 不在 G10 的 PATH，请确认 zero 能按该路径调用工具。" -ForegroundColor Green
