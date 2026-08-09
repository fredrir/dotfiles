---
name: hardware-optimization
description: >-
  Hardware-aware decisions for fredrir's compute environments: a local Arch workstation
  with a Ryzen 7 9800X3D, 32 GB DDR5, an RTX 5070 Ti with 16 GB VRAM, and no swap;
  CPU-only production servers; optional hosted GPUs; and external inference APIs. Use
  whenever choosing or sizing an ML model, tuning GPU memory, context, concurrency, or
  batch settings, deciding where a workload should run, diagnosing OOM or performance
  problems, setting container resource limits, or when output quality is constrained by
  available compute.
metadata:
  version: "2.0"
---

# Hardware optimization

Fit work to the hardware that actually exists. Use the stable baseline in this skill for
architecture decisions, but inspect live utilization, running services, installed
software, and provider pricing at the time of every recommendation.

## Stable local baseline

### Workstation (`archpc`)

- **CPU**: AMD Ryzen 7 9800X3D, 8 cores and 16 threads, approximately 5.2 GHz boost,
  with 96 MB X3D L3 cache. Treat 16 threads as the upper CPU parallelism budget and
  leave capacity for the desktop during interactive work.
- **RAM**: 32 GB DDR5-6000 CL30, approximately 30.5 GiB usable. Swap is disabled, so
  exhausted memory leads to OOM kills instead of gradual swap slowdown.
- **GPU**: NVIDIA GeForce RTX 5070 Ti with 16 GB GDDR7 VRAM, Blackwell compute
  capability 12.0 (`sm_120`). The AMD integrated GPU is not a compute target.
- **Storage**: a 2 TB NVMe contains a 366 GiB `/` partition and an 823 GiB `/home`
  partition. A separate 1.8 TiB SATA hard drive is mounted at `/storage`.
- **OS**: Arch Linux with a rolling kernel, NVIDIA driver, CUDA toolkit, and user-space
  ML stack.
- **Ports**: Ollama conventionally serves on `11434`. Check listeners before assigning an
  OpenAI-compatible model server; prefer `8001` on this workstation to avoid common
  conflicts on `8000`.

Keep latency-sensitive model weights, active datasets, container layers, and build
caches on NVMe. Use `/storage` for cold datasets, archives, completed artifacts, and
other capacity-oriented data that does not need NVMe latency.

Partition totals are stable baseline facts. Never record current used, available, or
free storage, RAM, or VRAM in this skill; those values must be measured live.

## Live checks before sizing

Run only the checks relevant to the task:

```sh
lscpu
free -h
swapon --show
nvidia-smi
lsblk -o NAME,SIZE,MODEL,TRAN,ROTA,FSTYPE,MOUNTPOINTS
df -hT
ss -ltnp
docker system df
docker ps --format '{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
ollama list
ollama ps
```

For Python workloads, verify the runtime rather than inferring compatibility from the
host driver:

```sh
python -c 'import torch; print(torch.__version__, torch.version.cuda, torch.cuda.get_arch_list(), torch.cuda.get_device_capability() if torch.cuda.is_available() else None)'
```

The driver-reported CUDA version, installed toolkit, Python wheel runtime, and container
runtime are different layers. A new host toolkit does not make an older binary support
`sm_120`. CUDA 12.8 introduced Blackwell compiler support, but prefer a current stable
wheel or image that explicitly includes `sm_120` instead of prescribing one permanent
CUDA build tag. Confirm support from the runtime itself.

## Where a workload should run

Walk this ladder and stop at the first option that meets quality, memory, latency, and
cost requirements:

1. **Local workstation**: default for development and interactive inference when the
   model, KV cache, vision components, and concurrency fit within 16 GB VRAM with
   desktop headroom.
2. **Hosted GPU**: use when the model does not fit locally, a quality-critical model is
   larger than the local card, or a batch would monopolize the workstation for hours.
3. **External inference API**: use for frontier quality, occasional workloads, or cases
   where provisioning and warming a hosted GPU is not worthwhile.
4. **CPU-only production servers**: do not use for GPU-oriented LLM or VLM inference.
   They are application hosts, not inference machines.

If hardware limits force a smaller model, shorter context, lower image resolution, or
more aggressive quantization, say so explicitly. Present the quality-preserving paid
option rather than silently choosing a degraded local configuration.

## VRAM fit estimation

Use this as a conservative first-pass heuristic, not a guarantee:

```text
usable_vram_gb = total_vram_gb * 0.85 - 1.5
estimated_instance_vram_gb = params_b * bytes_per_param * 1.4

fp32 = 4.0 bytes per parameter
fp16 or bf16 = 2.0
q8 or int8 = 1.0
q6 = 0.75
q5 = 0.65
q4 or int4 = 0.50
q3 = 0.40
q2 = 0.30
```

The multiplier approximates runtime overhead, but real usage depends on architecture,
engine, attention implementation, image encoder, projector, activation sizes, context,
batching, and KV-cache precision. Treat model metadata and observed engine allocation as
stronger evidence than parameter-count math.

Practical first calls for the 5070 Ti:

- Up to roughly 4B parameters at fp16 or bf16 is normally comfortable.
- Up to roughly 8B at q8 is normally comfortable.
- Up to roughly 12B at q4 or q5 is normally comfortable at moderate context.
- A 7B to 8B fp16 model does not fit fully under the conservative budget.
- A 32B model does not fit fully on the GPU under this heuristic even at q2; CPU
  offloading is possible but usually slow enough that a hosted GPU or API is preferable.
- VLMs need extra allowance for the vision encoder, projector, image tokens, resolution,
  and concurrent page or image processing.

KV cache grows with context length, concurrent sequences, and parallel requests. Long
context and high concurrency can make a weight-compatible model fail at runtime.

## Local LLM and VLM serving

### Ollama

Ollama is the default simple server for quantized local LLMs and VLMs. It conventionally
serves at `http://localhost:11434`.

Inspect rather than assume its state:

```sh
curl -fsS --max-time 3 http://127.0.0.1:11434/api/version
ollama list
ollama show MODEL
ollama ps
systemctl show ollama -p ExecStart -p Environment --no-pager
```

Use `ollama show` to confirm parameter count, quantization, and maximum context. Use
`ollama ps` to confirm whether a loaded model is fully on GPU, partially offloaded, and
how much context is allocated.

`OLLAMA_NUM_PARALLEL` multiplies the memory required for the configured context. Flash
Attention and a quantized KV cache can reduce context memory, but do not turn concurrency
into free capacity. Multiple models can remain resident only while their complete
allocations fit.

### OpenAI-compatible local servers

Use an OpenAI-compatible engine such as vLLM when continuous batching, higher throughput,
or a model unsupported by Ollama justifies a dedicated server. The local port convention
is `127.0.0.1:8001`.

Do not assume a server, container, image, or package exists merely because a configuration
points at it. Verify all four layers:

```sh
ss -ltnp '( sport = :8001 )'
curl -fsS --max-time 3 http://127.0.0.1:8001/v1/models
docker ps --format '{{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
python -c 'import importlib.util; print(importlib.util.find_spec("vllm"))'
```

Pin explicit image versions for reproducibility and choose a CUDA build that supports
`sm_120`. Treat `--gpu-memory-utilization` as an engine allocation ceiling, not proof
that unrelated GPU workloads can safely coexist. Set sequence and context limits from
measured VRAM, then load-test before increasing them.

### Concurrency and co-residency

- Inline inference jobs need a machine-wide concurrency cap or GPU lock. Start with one
  GPU-heavy job when another model is resident.
- Served inference is bounded by client concurrency, server sequence limits, queueing,
  KV-cache capacity, and available VRAM. Effective concurrency is the lowest of those
  limits, not simply workers multiplied by requests per worker.
- Auto-sizing based on total VRAM overestimates capacity when the desktop or another
  server already holds VRAM. Use current free VRAM and an explicit reserve.
- Prefer queueing over CPU fallback when latency is acceptable. Use CPU fallback only
  when the model and deadline make it reasonable.
- After an OOM, release stale model processes or caches and re-measure before retrying
  with lower context, concurrency, resolution, or batch size.

## CPU-only production environments

Production hosts are shared application environments. Discover their current CPU, RAM,
swap, disk, containers, and configured resource limits before recommending a change.
Never assume a cloud instance label or an old infrastructure document still describes
the live machine.

Do not place local LLM or VLM inference on these servers. CPU inference can consume all
cores, increase request latency, and compete with databases, workers, and ingress
services while still delivering poor model throughput.

Container memory limits cap individual containers but do not reserve memory for them.
Uncapped services, the kernel, page cache, and capped containers all share host RAM. CPU
limits likewise protect the host only when every competing service is accounted for.
Verify swap rather than assuming it exists; without swap, aggregate overcommit ends at
the host OOM killer.

## Hosted GPU safety and throughput

Only fredrir controls billable GPU lifecycle operations. Do not execute provider CLI or
API calls that create, start, stop, resize, or terminate hosted compute, and do not run a
local command that can start it implicitly. Read-only status checks are acceptable when
credentials and resource identifiers are already available.

Prepare commands and configuration for fredrir, state the required GPU memory and
expected behavior, then wait for confirmation that the resource is running before using
it. Before every recommendation, retrieve current compute pricing, billing granularity,
storage pricing, availability, and stop semantics from the provider. Do not preserve
provider prices in this skill.

Account for the full lifecycle:

- Image pull, model download, compilation, and warmup consume billable time.
- Persistent storage may continue billing after compute stops.
- Stopping and terminating can have different data-retention behavior.
- A watchdog or maximum runtime protects against hung jobs, but only if it is actually
  configured and authorized to stop the resource.
- Confirm the final provider state after a run instead of assuming cleanup succeeded.

For a running hosted GPU, optimize for total wall-clock completion. Keep the pipeline
fed, prefetch input, batch enough work to saturate the GPU, and raise client concurrency
only to the measured server limit. Avoid both idle paid compute and concurrency high
enough to trigger OOM retries or request collapse.

## External inference APIs

Compare an API against hosted compute using current model quality, token or image price,
rate limits, privacy requirements, retry behavior, and expected workload size. Do not
assume one gateway or provider is always the route. Prefer direct or gateway access based
on current availability and total cost for the specific model.

Use APIs for workloads where frontier quality matters, demand is bursty, or local and
hosted setup costs dominate the actual inference time. Keep model identifiers, prices,
and provider-specific switches in project configuration rather than this hardware skill.

## Memory and storage pitfalls

- With local swap disabled, bound RAM-heavy parallelism and monitor long-running jobs.
- Leave desktop CPU, RAM, and VRAM headroom during interactive local work.
- Docker data lives on `/`; inspect `docker system df` before adding large images or
  build caches.
- Do not recommend pruning Docker data without identifying reclaimable objects and
  receiving approval for the destructive operation.
- Keep active model caches on NVMe. Move cold artifacts to `/storage` when HDD latency is
  acceptable.
- Avoid writing unbounded temporary files, rendered pages, or model outputs to `/tmp` or
  a container writable layer.

## Recommendation checklist

Every hardware-sensitive recommendation should state:

1. The target environment and why it is the first option that fits.
2. The model, precision or quantization, context, image resolution, and estimated memory.
3. The proposed concurrency, batching, queueing, and co-residency limits.
4. Which live checks were used and which assumptions remain unverified.
5. The expected quality or latency compromise.
6. The escalation path when the local configuration does not fit.
7. Current cost and cleanup requirements when paid infrastructure or APIs are involved.
