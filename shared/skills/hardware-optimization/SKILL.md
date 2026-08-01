---
name: hardware-optimization
description: >-
  Hardware-aware decisions for fredrir's compute environments: local Arch workstation
  (Ryzen 7 9800X3D, 32 GB DDR5, RTX 5070 Ti 16 GB VRAM, swap disabled), CPU-only Hetzner
  prod servers, and on-demand RunPod GPU pods. Use this whenever choosing or sizing an
  ML model, tuning GPU/VRAM/concurrency/batch settings, deciding where a workload should
  run (local vs prod vs RunPod vs external API), diagnosing OOM or performance problems,
  setting Docker/compose resource limits, or when a feature's quality is gated on
  compute — even if the user never says the word "hardware".
---

# Hardware optimization

Work happens in exactly three places, plus one paid escape hatch. Fit the work to the
hardware instead of assuming a generic datacenter: the local GPU has 16 GB of VRAM, the
prod servers have no GPU at all, and RunPod pods cost money by the hour.

## The environments

### Local workstation (`archpc`) — where dev happens

- **CPU**: AMD Ryzen 7 9800X3D — 8 cores / 16 threads, ~5.2 GHz boost, 96 MB X3D L3.
  Excellent single-thread performance; treat 16 threads as the parallelism budget.
- **RAM**: 32 GB DDR5-6000 CL30 (~30.5 GiB usable). **Swap is disabled** — memory
  pressure ends in OOM kills, not graceful slowdown. The KDE desktop session typically
  holds 8–12 GiB, so plan around ~18–20 GiB free for work. Check with `free -h`.
- **GPU**: NVIDIA RTX 5070 Ti, **16 GB VRAM** (Blackwell, sm_120). Blackwell needs
  CUDA 12.8+ builds — use `cu128` wheels/images (e.g. `docling-serve-cu128`); older
  CUDA binaries fail with "no kernel image" errors. There is also an AMD iGPU in the
  Ryzen — ignore it for compute. The desktop session may hold some VRAM; check real
  headroom with `nvidia-smi` before sizing anything.
- **Storage**: 2 TB PCIe 4.0 NVMe split into `/` (78 GB — Docker images live here,
  watch for bloat), `/home` (118 GB), and `/home/fredrir/llunde` (393 GB — put large
  artifacts, datasets, and model caches under this mount when possible).
- **OS**: Arch Linux (rolling). Port 8000 is taken by Portainer — local vLLM serves
  on **8001**.

### Hetzner prod — CPU only; never suggest running inference here

- **`llunde-parser`** (pyparser prod, `parser.llunde.no`, 95.217.135.164): **CCX23 —
  4 dedicated vCPU / 16 GB RAM / 160 GB disk** (~€39/mo; matches
  `infra/pyparser/server.tf` and `PROD.md` — the `docker-compose.prod.yml` header
  saying "CCX22" is a stale label, its specs are right). Runs Postgres + review app +
  both workers + cloudflared under Docker Compose, all sharing that 16 GB. Compose
  caps: `worker-extract` 6g / 3 CPUs, `worker-light` 2g.
- **`ubuntu-llunde`** (llunde.no app): **CPX22 — 2 vCPU / 4 GB RAM / 80 GB disk**.
  A **completely separate application** that has nothing to do with pyparser beyond
  living in the same Hetzner project — when optimizing pyparser, do NOT optimize for
  or otherwise account for this box. It is listed here only so it isn't mistaken for
  the parser server. Same no-inference rule applies.
- Both are behind Cloudflare Tunnels with SSH-only firewalls. `PYPARSER_VLM_MODELS=""`
  in prod is **deliberate**: no GPU means VLM variants would grind on CPU, so they are
  disabled rather than slow.

### RunPod — on-demand GPU, existing tooling, costs money per hour

Fully built out in pyparser; never propose new cloud infra for GPU bursts. See the
playbook below.

## Where should a workload run?

Walk this ladder top-down and stop at the first rung that fits:

1. **Local** (free, fastest iteration) — the model fits in 16 GB VRAM with headroom
   (see fit math below). Serve via ollama (`:11434`) or the vLLM container (`:8001`).
2. **RunPod pod** (paid hourly, **started/stopped by fredrir only** — see the
   operator rule below) — the model is too big for 16 GB, or it's a quality-critical
   batch run that would monopolize the desktop GPU for hours.
3. **External API** (OpenRouter via `PYPARSER_USE_EXTERNAL_API=1`) — frontier-model
   quality needed, or a one-off where pod spin-up isn't worth it.
4. **Prod servers: never.** They are sized for serving the app, not inference.

If a feature's quality is limited by local hardware, say so explicitly and propose the
RunPod route (or external API) — don't silently ship the degraded local option, and
don't pretend the 16 GB card can do what it can't.

## VRAM fit math (matches pyparser's `extract/vram.py`)

```
usable ≈ 16 GB × 0.85 − 1.5 GB reserve ≈ 12 GB
weight_gb ≈ params_b × bytes_per_param × 1.4 (runtime overhead)
bytes_per_param: fp32=4, fp16/bf16=2, q8=1, q6=0.75, q5=0.65, q4=0.5
```

Practical calls for the 5070 Ti:

- **Comfortable**: ≤4B fp16, ≤8B q8 (~11 GB), ≤12B q4/q5 (gemma3:12b q4 ≈ 8.4 GB).
- **Does not fit**: 7–8B at fp16 (~20 GB with overhead) — quantize to q8 instead.
- **32B-class**: only at q3-or-below with offloading — slow and degraded. Recommend
  RunPod or the external API instead of pretending this is viable.
- KV cache and long contexts eat into headroom — leave margin for context-heavy work.

## Local serving conventions

- **ollama** at `http://localhost:11434` (catalog endpoints `vlm_local` / `llm_local`);
  tags like `gemma3:12b`, `qwen2.5vl:7b`, `glm-ocr:q8_0`.
- **granite-docling on vLLM** in docker (`granite-vllm` container) at
  `127.0.0.1:8001` with `--gpu-memory-utilization 0.10` — tiny 258M model, deliberately
  capped so it coexists with everything else. Health: `curl -s localhost:8001/v1/models`.
- **Concurrency knobs** (pyparser): `PYPARSER_GPU_MAX_CONCURRENT=0` (default)
  auto-sizes inline GPU slots — but the auto-sizing reads **total** VRAM
  (`auto_inline_capacity()` in `extract/backends.py`), not currently-free VRAM. When
  another model is co-resident (e.g. gemma3:12b holding ~8–9 GB in ollama), auto
  overestimates: set an explicit cap (`1` = exclusive lock, `2` is a sane
  co-residency value) for inline GPU runs alongside a resident LLM. Served/API
  models skip this gate entirely — their effective concurrency is
  `workers × vlm-concurrency`. `gpu_min_free_gb=3.5` is the runtime backstop
  (individual conversions fall back to CPU when free VRAM is short), and
  `docling_vlm_concurrency=4` caps in-flight page requests per remote conversion.

## RunPod playbook (pyparser)

**Operator rule: only fredrir runs RunPod — never you.** Do not execute pod lifecycle
commands (`pyparser-llm pod start|stop`), and do not run anything that can start a pod
*implicitly*: `pyparser-extract` auto-starts configured pods through its bracket when a
runpod_pod-backed model is active. Pods bill by the hour, so the human keeps the
on/off switch. Your job is to prepare the exact command(s), state the expected cost,
and hand them over — then wait for confirmation of pod state before proceeding.
`--no-pod-start` is the safe flag once fredrir says a pod is already up.

**On a pod, maximum throughput is priority #1.** RunPod bills by the second, so the
cheapest run is the one that saturates the GPU and finishes fastest. This inverts the
local mindset: the conservative co-residency caps that protect the desktop do not
apply here. Batch aggressively, raise client concurrency to match the server's limits
(e.g. vLLM's `--max-num-seqs`), keep the pipeline fed end-to-end (prefetch), and never
let a pod idle between phases — optimize for wall-clock completion, not gentleness.

- **Lifecycle**: `pyparser-llm pod status|start|stop` (`start --wait` polls
  `/v1/models`). Pod URL format: `https://{pod_id}-{port}.proxy.runpod.net`.
- **Automatic bracketing**: `pyparser-extract` starts configured pods on entry and
  stops them on any exception; `--keep-pod-running` keeps them alive after success,
  `--no-pod-start` assumes they're already up. A `PodWatchdog` (default 600 s stall
  timeout) force-stops pods and exits if progress stalls — it exists because idle
  pods burn money.
- **Requires** `RUNPOD_API_KEY` (or `PYPARSER_RUNPOD_API_KEY`). Without it, a
  runpod_pod model runs **unguarded** — nothing will auto-stop the pod. Always confirm
  the key is set before a pod-backed run.
- **Config**: `pyparser/llm-providers.toml` — `[endpoints.*]` with `kind="runpod_pod"`,
  `pod_id`, `port` (default 8000). docling-serve pods use image
  `quay.io/docling-project/docling-serve-cu128`, port 5001, pointed at via
  `PYPARSER_DOCLING_SERVE_URL`.
- When recommending a pod run, mention the cost dimension (`PodStatus.cost_per_hr` is
  surfaced) and that pods should be stopped when done.

## Pitfalls worth repeating

- **Swap disabled locally**: a leaky long batch job dies by OOM kill with no warning.
  For multi-hour runs, sanity-check `free -h` and prefer bounded concurrency.
- **`mem_limit` in compose does not reserve memory** — on prod, `worker-extract` 6g +
  `worker-light` 2g + an uncapped review app + Postgres share 16 GB. Check the sum
  before raising limits or adding services; past physical RAM it's the host OOM
  killer, not Docker, that decides who dies.
- **`/` is only 78 GB**: suggest `docker system prune` when images pile up; keep
  HF/model caches on the big `/home/fredrir/llunde` mount.

## Repo pointers

- Model/endpoint catalog: `pyparser/llm-providers.toml`
- Config levers (`PYPARSER_*`): `pyparser/src/pyparser/config.py`
- VRAM math: `pyparser/src/pyparser/extract/vram.py`, `extract/backends.py`
- RunPod bracket/watchdog: `pyparser/src/pyparser/llm/runpod.py`, `extract/cli.py`
- Pod CLI: `pyparser/src/pyparser/llm/cli.py`
- Prod compose + limits: `pyparser/docker-compose.prod.yml`
- Infra (Terraform, server types): `infra/pyparser/`, `infra/llunde/`
- Deploy: `.github/workflows/deploy-pyparser.yml`, `pyparser/deploy-remote.sh`
