<p align="center">
  <img src="static/favor.png" width="112" alt="dreampaper logo">
</p>

<h1 align="center">DreamPaper</h1>

<p align="center"><strong>Local paper figures and academic slides — template in, publication-ready image out.</strong></p>

<p align="center">
  <img alt="Python 3.10+" src="https://img.shields.io/badge/Python-3.10%2B-3776AB?logo=python&logoColor=white">
  <img alt="FastAPI 0.115+" src="https://img.shields.io/badge/FastAPI-0.115%2B-009688?logo=fastapi&logoColor=white">
  <img alt="React 19" src="https://img.shields.io/badge/React-19-61DAFB?logo=react&logoColor=black">
  <img alt="Vite 7" src="https://img.shields.io/badge/Vite-7-646CFF?logo=vite&logoColor=white">
  <img alt="TypeScript 5.8" src="https://img.shields.io/badge/TypeScript-5.8-3178C6?logo=typescript&logoColor=white">
  <img alt="Tauri 2" src="https://img.shields.io/badge/Tauri-2-24C8D5?logo=tauri&logoColor=white">
  <img alt="Rust stable" src="https://img.shields.io/badge/Rust-stable-000000?logo=rust&logoColor=white">
  <a href="LICENSE"><img alt="License PolyForm Noncommercial 1.0.0" src="https://img.shields.io/badge/License-PolyForm%20Noncommercial%201.0.0-F5A623"></a>
</p>

<p align="center">中文版：<a href="README.md">README.md</a></p>

---

## Why dreampaper

- **Template-driven** — figures use PaperBananaBench few-shot refs; slides lock layout and palette from your master image
- **Two-stage design** — structure / master first, then content, less template-copy and style drift
- **Bring your own models** — separate Design / Implement / Search profiles (OpenAI, Anthropic, image2, banana2, …)
- **Stage hooks** — event hooks stream the design log; context hooks inject contracts, inventories and evidence into each stage prompt as pluggable sections
- **Case memory + advisor** — design products are kept as cases, recalled by CJK-bigram FTS5 similarity, and an advisor role compares them into layout advice; rate a task good / fair / poor to steer later recalls
- **Visual grounding** — figure and slide jobs alike detect products and instruments in the input, search appearance cues, and push the drawing model toward real objects instead of labeled boxes (no master screenshot ever goes into a search request); the step is skipped when no search model is configured
- **Fully local** — config and outputs under `~/.dreampaper/`; keys never enter the repo

---

## Gallery

### Web UI

| Figure | Slide | Settings |
| --- | --- | --- |
| ![figure ui](examples/ui/figure.jpg) | ![slide ui](examples/ui/slide.jpg) | ![settings ui](examples/ui/settings.jpg) |

| History | Design log |
| --- | --- |
| ![history](examples/desktop/history.jpg) | ![log](examples/desktop/log.jpg) |

### Paper figures

|  |  |  |  |
| --- | --- | --- | --- |
| ![fig1](examples/figure/paper_figure_1.png) | ![fig2](examples/figure/paper_figure_2.png) | ![fig3](examples/figure/paper_figure_3.png) | ![fig4](examples/figure/paper_figure_4.png) |

### Slides

|  |  |  |
| --- | --- | --- |
| ![sA1](examples/slide/slideA_1.png) | ![sA2](examples/slide/slideA_2.png) | ![sA3](examples/slide/slideA_3.png) |
| ![sB1](examples/slide/slideB_1.png) | ![sB2](examples/slide/slideB_2.png) | ![sB3](examples/slide/slideB_3.png) |
| ![sC1](examples/slide/slideC_1.png) | ![sC2](examples/slide/slideC_2.png) | ![sC3](examples/slide/slideC_3.png) |

---

## Desktop downloads

Prefer not to set up Python? Grab the [latest release](../../releases/latest). The desktop build embeds a Rust backend, so there is no separate server to start.

| File | Platform |
| --- | --- |
| `dreampaper-*-setup.exe` | Windows installer (creates a desktop shortcut) |
| `dreampaper-*-portable.zip` | Windows portable (extract the complete directory; do not move the EXE alone) |
| `dreampaper-*-x64-mac.dmg` | macOS Intel |
| `dreampaper-*-arm64-mac.dmg` | macOS Apple Silicon |
| `dreampaper-*-amd64.deb` | Ubuntu 22.04 or newer, x86-64 (Debian family) |

### First launch

There is no commercial signing certificate: Windows executables are unsigned; macOS uses ad-hoc signing without Developer ID notarization. The OS may show a warning:

- **macOS**: double-clicking reports an unverified developer. Right-click the app → Open → Open again. One time only.
- **Windows**: SmartScreen shows "Windows protected your PC". Click "More info" → "Run anyway".
- The **Windows portable** build requires [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/). Install it if missing, or use the installer. Keep the main EXE, OCR sidecar, `ort/`, fonts and other files together.
- **Linux**: `sudo apt install ./dreampaper-*-amd64.deb`. The package declares its `libwebkit2gtk-4.1-0`, `libgtk-3-0` and `libgomp1` dependencies for apt to resolve. It is built on Ubuntu 22.04 x86-64 so it installs on 22.04 and newer. No RPM or AppImage is published.

All five artifacts include the OCR engine, but not the models. Download the approximately 133 MiB model package in Settings once; detection/recognition files fall back between PaddlePaddle's official ModelScope and Hugging Face mirrors, with pinned file sizes and SHA-256 verification. Recognition then works offline without Python or a separate ONNX Runtime installation. Release CI validates actual installed/extracted packages and creates only a draft after all five pass.

### Desktop builds

Build on the target platform with Node.js 22, Rust and the platform toolchain (Visual Studio 2022 C++ on Windows, Xcode on macOS, the Tauri dependencies such as `libwebkit2gtk-4.1-dev` and `libgtk-3-dev` on Linux). Install Python 3.12, `cmake==4.1.2` and `ninja==1.13.0` for the ORT build only; these tools are not shipped. Run `npm ci`, `npm run gate:ort`, `npm run sidecar`, then the Rust tests and `npm run tauri:build`. The engine must be staged before Tauri tests compile.

For local desktop debugging use `npm run tauri:dev`: it stages a debug build of the OCR sidecar into `src-tauri/binaries/` and makes sure the `runtime/ort` resource directory exists, since Tauri's build script requires both `externalBin` and the resource path. Without a previous `gate:ort` there is no ONNX Runtime on disk, so OCR is unavailable while everything else works; run `gate:ort` and `sidecar` above when you need the real engine.

CI builds CPU ORT from a pinned commit on each platform and verifies its per-build file manifest before packaging. Missing engines cannot be bypassed. Manual workflow runs upload artifacts only; tag runs create a draft after all gates pass. Runtime manifests and OCR reports are available as Actions artifacts.

The final DMG is assembled by `scripts/bundle.mjs`, which grants the ad-hoc library-loading exception only to the OCR sidecar and re-signs the outer app. The main executable retains Hardened Runtime; do not distribute the intermediate Tauri app instead.

The macOS deployment target is 13.0. Passing on modern CI runners does not establish minimum-version compatibility; macOS 13.0 and the minimum Windows version require separate installed-package testing.

On Linux, ORT is built natively from the pinned commit and checked for target architecture, leftover build-machine paths and dynamic dependencies. The `.deb` is produced by Tauri; the runtime dependencies live in `src-tauri/tauri.linux.conf.json`, and the release gate checks the built package's control fields against that list (it does not assume whether upstream derives defaults). Linux gates run with `LD_LIBRARY_PATH`, `LD_PRELOAD` and `LD_AUDIT` cleared. The build baseline is Ubuntu 22.04 x86-64, and the package runs on 22.04 and newer.

### Workbench (desktop)

The **Workbench**, below History in the left nav, fixes garbled text or typos in generated images: open an image from a result card, history or disk, box-select the faulty region to cover it with the sampled background color and recognize the text into an editable layer, then adjust wording and layout, crop on whole source pixels, and export a lossless PNG at the source resolution. Source images are always read-only; each image's edits live in their own project and can be restored any time.

- **OCR is fully offline**: the engine (pure-Rust sidecar + ONNX Runtime) ships with the installer; the PP-OCRv6 medium detection/recognition models and the text-orientation classifier (about 133 MiB, sources and checksums pinned in the app) download on first use, and afterwards neither images nor results leave the machine. The settings page shows the engine version and can update or remove the models.
- Without the models, solid-color patching, cropping and manual text still work.
- Workbench data lives under `workbench/` (projects and source snapshots) and `ocr/` (models) in the app data directory; the settings page reports usage and offers a safe cleanup that only drops unreferenced snapshots.

The desktop build stores config and outputs in the OS app-data directory rather than `~/.dreampaper/`:

| Platform | Path |
| --- | --- |
| macOS | `~/Library/Application Support/com.dreampaper.app/` |
| Windows | `%APPDATA%\com.dreampaper.app\` |
| Linux | `~/.local/share/com.dreampaper.app/` (follows `XDG_DATA_HOME`) |

---

## Template library (paper figures)

This repo **does not ship** PaperBananaBench. Figure mode needs it locally.

1. Download [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench) (~266MB)
2. Unzip at the repo root:

```text
dream-paper/
  PaperBananaBench/
    diagram/
      ref.json
      images/…
    plot/
      ref.json
      images/…
```

3. Restart the backend, then pick 1–3 templates in the Figure page

> Skip this if you only use Slide mode.

**The desktop build** does not read the repo directory. Import from the Templates page instead:

- **Import pack**: extract PaperBananaBench, click "Choose directory…", and select the extracted directory (the one containing `diagram/` and `plot/`) to import everything at once
- **Import image**: add your own templates one at a time with kind / category / description

Imported templates are copied into the app data directory, so switching branches or deleting the repo does not affect them.

---

## Run locally

```bash
python3 -m venv .venv
source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -r requirements.txt
npm install
```

```bash
# terminal 1 — API
uvicorn backend.app.main:app --reload --host 127.0.0.1 --port 8000

# terminal 2 — UI
npm run dev
```

Open http://127.0.0.1:5173

---

## Model configuration

Three roles are configured separately, each with its own protocol / URL / model / key. Saving on the Settings page writes them to `~/.dreampaper/config.json`.

1. **`design model` (planner, needs multimodal input)** — reads the uploaded material and images in depth, arranges the content and layout of the target image, and produces the final drawing description. Strong style consistency comes from the template image. The upstream model must accept image input; OpenAI and Anthropic protocols are supported.
2. **`implement model` (renderer)** — takes the `design model` output and produces the final image. Supports OpenAI `v1/images/generations` and the gemini protocol; works with gpt-image-2 and nano-banana-2.
3. **`search model`** — extracts real-world objects from the uploaded material and images: hardware (radar, cameras, drones, …) and software products (Claude Code, Codex, Pi, …). It reuses real reference images from the web to guide the `design model`, suppressing invented visuals and enriching the result. Point it at xAI search (a Grok search model) or Tavily; DuckDuckGo is the default.

### Protocol dropdown

| Role | Protocol | Endpoint | Typical setup |
| --- | --- | --- | --- |
| design | `openai_responses` (default) | `{URL}/v1/responses` | `https://api.openai.com` + `gpt-5.4` |
| design | `openai_chat` | `{URL}/v1/chat/completions` | any OpenAI-compatible gateway |
| design | `anthropic_messages` | `{URL}/v1/messages` | `https://api.anthropic.com` |
| implement | `image2` (default) | `{URL}/v1/images/generations`, or `/v1/images/edits` when reference images are attached | `https://api.openai.com` + `gpt-image-2` |
| implement | `banana2` | `{URL}/{version}/interactions` | gemini-protocol gateway + `nano-banana-2` |
| search | `duckduckgo` (default) | DuckDuckGo no-JS page | no key and no URL (both fields are hidden); the endpoint is fixed in code |
| search | `tavily` | `{URL}/search` | `https://api.tavily.com`, API key required |
| search | `grok_search` | `{URL}/v1/chat/completions` | OpenAI-compatible gateway (e.g. grok2api) + a model ID that carries search itself (`grok-build-0.1`) |

> The URL only needs the host; `/v1` is appended automatically when missing. `banana2` is the exception — its version comes from the Version field (default `v1beta`).

Search settings are stored **per protocol**: `duckduckgo`, `tavily` and `grok_search` each keep their own URL, model and key. Switching the protocol in the settings selects that profile — it neither carries the previous protocol's values over nor loses what you already filled in, keys included. The form only shows the fields a protocol actually reads: `duckduckgo` needs nothing at all (its endpoint is fixed and it takes no key, so neither field is shown), `tavily` never reads a model so that field is hidden, and only `grok_search` shows all three. Upgrading from an older build repairs the leftovers: a profile that still carries another protocol's endpoint and model (older builds had a single search profile, so switching protocols left the values behind) moves over to `grok_search` in full, and the protocol it came from gets a fresh default profile — nothing has to be retyped.

`duckduckgo` is the keyless protocol: it reads DuckDuckGo's no-JS page. That page is touchy about automation — sending only a `User-Agent` gets flagged and answers HTTP 202 with an anti-bot page that contains no results at all; it needs the browser-only `Sec-Fetch-*` and `Upgrade-Insecure-Requests` headers, and a busy exit IP gets throttled anyway. So a challenge is never disguised as "no sources": it fails the job with a message naming `tavily` and `grok_search` as the alternatives, and for regular use those two (real APIs) are the ones to pick.

A failed search — wrong route, rejected key, rate limit, anti-bot page — is treated the same way for **every** protocol: the job fails with the reason, instead of silently degrading to "this subject has no sources". Otherwise the infrastructure problem is invisible and the figure gets drawn with no grounding at all. A search that genuinely returns zero results still degrades as before (the prompt has a dedicated branch for it). Any failed job can be retried: failed searches are re-sent, while the steps that already finished replay from the cache.

`grok_search` is exactly "an OpenAI-compatible chat completions endpoint whose upstream model runs the search tools itself" — [grok2api](https://github.com/chenyme/grok2api) is one such gateway. The model is the deployment's own model ID (for example `grok-build-0.1`, which carries the search tools); no prefix is required and whether it can search is the server's answer. Requests are always non-streaming and set `tool_choice: required`, i.e. the request asks for at least one tool call. The request always declares both `x_search` and `web_search`: whichever is missing is appended, entries already configured are kept, nothing is duplicated. Every call's query and the results it received are recorded on the job and shown in the result card's "Generation steps" list, one collapsible row per round in execution order (a "Query" block and a "Query results" block); failed calls are recorded too. When a model cannot use those tools, the deployment sometimes answers HTTP 400 `A tool_choice was set on the request but no tools were specified` — which is exactly why the client cannot assert what ran, and why any failed request (route, auth or HTTP error) fails the job instead of degrading into a normal-looking "no sources" result.

### Image parameter dropdowns

`implement` with `image2`:

| Field | Options | Default |
| --- | --- | --- |
| Size `size` | `auto` / `16:9` / `1024x1024` / `1200x675` / `928x1664` / `3000x1000` | `1200x675` |
| Quality `quality` | `auto` / `low` / `medium` / `high` / `hd` | `auto` |
| Format `output_format` | `png` / `jpeg` / `webp` | `png` |
| response `response_format` | `url` / `b64_json` | `url` |

`implement` with `banana2`:

| Field | Options | Default |
| --- | --- | --- |
| Ratio `aspect_ratio` | `16:9` / `4:3` / `1:1` / `3:2` | `16:9` |
| Sharpness `image_size` | `1K` / `2K` / `4K` | `4K` |
| Quality `thinking_level` | `minimal` / `high` | `high` |
| Format `mime_type` | `image/png` / `image/jpeg` / `image/webp` | `image/png` |
| Version `api_version` | free text | `v1beta` |

### Shared and runtime parameters

| Field | Role | Range | Default / suggestion |
| --- | --- | --- | --- |
| Timeout (s) | all | 5–1800 | design 120; implement 600–900 (sync image APIs are slow); search 15 |
| Retries | all | 0–8 | design 2 / implement 3 / search 1 |
| Stream | design | on / off | off |
| Results | search | 1–8 | 3 |
| Proxy | global | URL or port | `http://127.0.0.1:7890`; empty means no explicit proxy (the `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` shell variables are never adopted automatically — only this field counts) |
| Plan workers | global (slides) | 1–20 | empty = follow page count |
| Image workers | global (slides) | 1–20 | empty = follow page count; start at 1 to reduce gateway 502s |

---

## Usage

1. **Settings** — configure Design / Implement (optional Search, proxy, concurrency), save  
2. **Figure** — pick templates → title + method → generate  
3. **Slide** — upload master → material + page count → generate  

The **Simple Mode** switch in the top bar runs the prompt pipeline only: it still needs the design model (and optionally search), never calls the drawing model, and requires no implement credentials. While it is on, the result card's generation list gains a **Final drawing prompt** row — the exact prompt the drawing model would have received, one for a figure and one per slide page in page order, with line breaks preserved and one-click copy. It is off by default, applies to both figure and slide jobs, and travels as a per-job snapshot: re-running history uses the current switch, while a job already running is unaffected. Simple-mode jobs therefore never have a finished image to show, so their history card previews the template that run used (click to zoom; the tooltip says "Preview: the template you picked") and carries a green **Simple** pill next to the title. The history filter bar also filters by run mode (all / standard / simple) alongside kind and status. If that template was deleted the card falls back to the plain icon instead of a broken image; a normal-mode job with no image simply did not finish, and never borrows the template as a stand-in.

| Path | Content |
| --- | --- |
| `~/.dreampaper/config.json` | model config |
| `~/.dreampaper/assets/` | uploads |
| `~/.dreampaper/jobs/` | job logs and images |

---

## Credits

Figure templates from [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench) ([PaperBanana](https://github.com/dwzhu-pku/PaperBanana)).

---

## License

Licensed under the [PolyForm Noncommercial License 1.0.0](LICENSE) — **noncommercial use is permitted, commercial use is not**.

| Use case | Permitted |
| --- | --- |
| Personal study, research, experiment, hobby projects | ✅ |
| Reading, modifying, redistributing the source | ✅ (must ship this license) |
| Internal production use at a company, paid or commercial services | ❌ |
| Selling this project or derivatives as a product | ❌ |
