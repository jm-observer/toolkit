# rag 服务部署与接入（g10）

完整设计见仓库 `docs/rag-service-design.md`。本文件是可照抄的操作步骤。

## 0. 前置：embedding 服务（bge-m3 / vLLM）

由用户部署。约定对外 `http://127.0.0.1:8092/v1/embeddings`，dim=1024：

```bash
docker run -d --name bge-m3 --runtime nvidia --gpus all \
  -e HF_ENDPOINT=https://hf-mirror.com \
  -v ~/.cache/huggingface:/root/.cache/huggingface \
  -p 8092:8000 \
  vllm/vllm-openai:v0.19.1-cu130-linuxarm64 \
  --model BAAI/bge-m3 --task embed \
  --gpu-memory-utilization 0.08 --max-num-seqs 8 --enforce-eager
# 验证：curl 127.0.0.1:8092/v1/embeddings -d '{"model":"BAAI/bge-m3","input":"测试"}'
```

> GB10 统一内存紧（gemma4 已占大头），`--gpu-memory-utilization` 压到 0.08；不够再降 0.05。

## 1. 配置

把 `rag.config.json` 放到 g10：`~/.config/zero/rag.config.json`。
`dim` 必须与 embedding 模型一致（bge-m3=1024）。`store.db_path` 相对 workspace
（`~/.config/zero`），即库落在 `~/.config/zero/rag.db`。

## 2. 部署 rag binary

沿用本仓 `deploy-g10.ps1`（Docker 交叉编译 aarch64 + scp 到 `~/.local/bin`），
target crate 选 `rag`。产物：`~/.local/bin/rag`。

## 3. 首次灌库 + 自测

```bash
# 全量扫描 knowledge/douyin/<抖音号>/transcripts/*.md 入库
~/.local/bin/rag ingest --config ~/.config/zero/rag.config.json
# → {"ingested":N,"skipped":M,"failed":K}

# 检索自测
~/.local/bin/rag search --config ~/.config/zero/rag.config.json --query "comfyui 工作流" --top-k 5
# → {"hits":[{external_id,chunk_index,text,score,metadata}, ...]}
```

## 4. 起 HTTP 服务（供 mcp-server 接入）

```bash
~/.local/bin/rag serve --config ~/.config/zero/rag.config.json --bind 127.0.0.1:8788
# 或对外（供局域网 mcp-server 直连）：--bind 0.0.0.0:8788
# 健康：curl 127.0.0.1:8788/healthz → {"status":"ok"}
# GET 检索：curl 'http://127.0.0.1:8788/v1/search?query=comfyui&top_k=5'
```

可用 systemd 用户级单元常驻（参考 douyin.service）。

## 5. mcp-server 接入（二选一，先测 SSRF）

把下面之一放进 mcp-server 的 workspace `tools.d/rag.toml`。

### 方案 A（推荐）：http 直连 rag serve

前提：rag serve `--bind 0.0.0.0:8788`，且 **mcp-server 不拦私有 IP**。
mcp-server 的 http 工具按 URL 编码传参，正好对应 rag 的 `GET /v1/search?query=..&top_k=..`。
base_url（`http://192.168.0.68:8788`）配在 mcp-server 的 config.toml http 段。

```toml
[[tools]]
name = "rag_search"
description = "在 douyin 向量知识库中按语义检索片段，返回 hits 数组（external_id/text/score/metadata）"
type = "http"
method = "GET"
path = "/v1/search"
timeout_secs = 30

[[tools.parameters]]
name = "query"
description = "自然语言检索词"
type = "string"
required = true

[[tools.parameters]]
name = "top_k"
description = "返回片段数，默认 5"
type = "number"
required = false
```

> ⚠️ 先实测：mcp-server SSRF 防护是否放行 `192.168.0.68`（私有段）。被拦则用方案 B。

### 方案 B（退路）：ssh 调 rag CLI

mcp-server 在 Windows 本机，经 ssh 跑 g10 的 rag CLI。要求 g10 免密登录。

```toml
[[tools]]
name = "rag_search"
description = "在 douyin 向量知识库中按语义检索片段，返回 hits 数组"
type = "command"
command = "ssh"
timeout_secs = 30
subcommands = ["fengqi@192.168.0.68", "~/.local/bin/rag", "search", "--config", "/home/fengqi/.config/zero/rag.config.json", "--namespace", "douyin"]

[[tools.parameters]]
name = "query"
description = "自然语言检索词（注意：经 ssh 透传，含空格的查询会被远端 shell 二次切词；中文查询通常无空格不受影响）"
type = "string"
required = true
arg = ["--query"]

[[tools.parameters]]
name = "top_k"
type = "number"
required = false
arg = ["--top-k"]
```

> 方案 B 的空格切词问题：若需稳健支持含空格查询，优先用方案 A，或后续给 rag CLI
> 加 stdin 读 query 的入口。

## 6. Claude Code 注册 mcp-server

项目级 `.mcp.json`（stdio 启动 mcp-server）：

```jsonc
{
  "mcpServers": {
    "tools": {
      "command": "D:\\git\\mcp-server\\target\\release\\mcp.exe",
      "args": ["--workspace", "<mcp-server workspace 目录>", "--stdio"]
    }
  }
}
```

连通后 Claude Code 侧出现 `rag_search` 工具，端到端验证：让 Claude 调用它检索 douyin 知识。
