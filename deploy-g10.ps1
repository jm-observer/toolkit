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

.EXAMPLE
    pwsh ./deploy-g10.ps1
    pwsh ./deploy-g10.ps1 -SkipBuild
#>
param(
    [string]$G10Host = "fengqi@192.168.0.68",
    [string]$DestDir = "~/.local/bin",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$RepoRoot = $PSScriptRoot
$Target = "aarch64-unknown-linux-gnu"
$Image = "huangjiemin/rust_aarch64-gcc_openssl:1.94.0_9.4.0_1.1.0l_llvm12.0.1"

# (crate package 名, 产物二进制名) —— 新增工具时在此追加一行即可。
#
# 注意 asr-server:依赖 sherpa-onnx 的 native shared libs(libsherpa-onnx-c-api.so /
# libonnxruntime.so / libsherpa-onnx-cxx-api.so)+ 运行期 ffmpeg。本脚本只 scp 裸
# 二进制,这些 .so 不会一并带过去——asr-server 的推荐部署形态是容器(crates/asr-server/
# Dockerfile + deploy/asr-tts/ 编排),裸二进制部署须自行把 sherpa .so 放进 G10 的
# LD_LIBRARY_PATH。保留此行是为了「与其它 bin 一起交叉编译验证产物可生成」。
$Bins = @(
    @{ Crate = "github_commit_info"; Bin = "github-commit-info" },
    @{ Crate = "hf_watcher";         Bin = "hf-watcher" },
    @{ Crate = "douyin";             Bin = "douyin" },
    @{ Crate = "asr_server";         Bin = "asr-server" }
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

    # 挂载仓库；用命名卷缓存 cargo registry，加速重复构建。
    # AR_ 显式补上（镜像只预置了 CC_/CXX_/LINKER）。
    docker run --rm `
        -v "${RepoRoot}:/work" `
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
    Write-Host "    scp $($b.Bin)"
    scp $local "${G10Host}:$DestDir/$($b.Bin)"
    if ($LASTEXITCODE -ne 0) { throw "scp $($b.Bin) 失败" }
}

# 保证可执行 + 打印版本确认。
$binNames = ($Bins | ForEach-Object { "$DestDir/$($_.Bin)" }) -join " "
ssh $G10Host "chmod +x $binNames"
Write-Host "==> 远端版本确认" -ForegroundColor Cyan
foreach ($b in $Bins) {
    ssh $G10Host "$DestDir/$($b.Bin) --version"
}

Write-Host "==> 完成。若 $DestDir 不在 G10 的 PATH，请确认 zero 能按该路径调用工具。" -ForegroundColor Green
