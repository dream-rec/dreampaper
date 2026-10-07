<p align="center">
  <img src="static/favor.png" width="112" alt="dreampaper logo">
</p>

<h1 align="center">DreamPaper</h1>

<p align="center"><strong>本地科研配图与学术幻灯片，从模板到成图一步到位。</strong></p>

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

<p align="center">English: <a href="README_EN.md">README_EN.md</a></p>

---

## 核心优势

- **模板驱动**：科研图以 PaperBananaBench 参考图作 few-shot；幻灯片以你上传的母版锁定版式与配色
- **两阶段设计**：先抽结构 / 母版，再填内容，减少「抄模板文案」与样式漂移
- **模型自选**：Design / Implement / Search 分配置，兼容 OpenAI / Anthropic / image2 / banana2 等协议
- **阶段 Hook**：事件 Hook 流式回传 design 日志，上下文 Hook 按阶段注入契约、清单、证据等 prompt section，可插拔
- **案例记忆 + advisor**：design 产物沉淀为案例，CJK 二元组 FTS5 召回相似历史任务，advisor 角色比对后给出版式建议；任务可打 优 / 良 / 差 反哺后续召回
- **视觉 grounding**：科研图与幻灯片都会从输入里识别产品与仪器，联网检索外观描述，引导制图模型画实物而非文字方框；未配置检索模型时自动跳过
- **全程本地**：配置与产物落在 `~/.dreampaper/`，密钥不进仓库

---

## 更新日志
**v0.2.0**
- 工作台：生成图片目前可直接在工作台进行编辑，边界修复文字乱码，字体不统一，颜色存在色度差等问题，支持无损导出；
- 离线 OCR：纯 Rust 辅助进程 + ONNX Runtime 在本机运行 PP-OCRv6，模型首次使用时下载并逐文件校验，图片与识别结果不出本机；
- 阶段 Hook：上下文注入显式化为可插拔的 Hook 链，进度日志列出每阶段注入项；
- 历史案例记忆：design 产物入库并以 CJK 二元组做 FTS5 召回，同模式相似任务最多取 3 条；
- advisor 角色：比对相似案例，输出可复用版式、术语映射与失败模式，注入设计阶段；
- 任务评分：成功后可标 优 / 良 / 差，写入案例记录供 advisor 参考；
- 多任务页：科研图与幻灯片各可开最多 8 个相互隔离的任务页；

## 效果展示

### Desktop

| 科研图 | 幻灯片 |
| --- | --- | 
| ![desktop1](examples/desktop/figure.jpg) | ![desktop2](examples/desktop/slide.jpg) |

| 工作台 | 
| --- | 
| ![workbench](examples/desktop/workbench.jpg) |

| 模版库 | 设置页 |
| --- | --- | 
| ![desktop3](examples/desktop/templates.jpg) | ![fig2](examples/desktop/settings.jpg) |

| 历史数据 | 日志显示 |
| --- | --- | 
| ![history](examples/desktop/history.jpg) | ![log](examples/desktop/log.jpg) |

### Web UI

| 科研图 | 幻灯片 | 设置页 |
| --- | --- | --- |
| ![figure ui](examples/ui/figure.jpg) | ![slide ui](examples/ui/slide.jpg) | ![settings ui](examples/ui/settings.jpg) |

### 科研绘图

|  |  |
| --- | --- | 
| ![fig1](examples/figure/paper_figure_1.png) | ![fig2](examples/figure/paper_figure_2.png) |
| ![fig3](examples/figure/paper_figure_3.png) | ![fig4](examples/figure/paper_figure_4.png) |
| ![fig5](examples/figure/paper_figure_5.png) | ![fig6](examples/figure/paper_figure_6.png) |

### 幻灯片

|  |  |  |
| --- | --- | --- |
| ![sA1](examples/slide/slideA_1.png) | ![sA2](examples/slide/slideA_2.png) | ![sA3](examples/slide/slideA_3.png) |
| ![sB1](examples/slide/slideB_1.png) | ![sB2](examples/slide/slideB_2.png) | ![sB3](examples/slide/slideB_3.png) |
| ![sC1](examples/slide/slideC_1.png) | ![sC2](examples/slide/slideC_2.png) | ![sC3](examples/slide/slideC_3.png) |
| ![sD1](examples/slide/slideD_1.png) | ![sD2](examples/slide/slideD_2.png) | ![sD3](examples/slide/slideD_3.png) |
---

## 桌面版下载

不想配 Python 环境的话，直接下[最新 Release](../../releases/latest)。桌面版内置 Rust 后端，无需单独启动服务。

| 文件 | 平台 |
| --- | --- |
| `dreampaper-*-setup.exe` | Windows 安装版（创建桌面快捷方式） |
| `dreampaper-*-portable.zip` | Windows 便携版（完整解压后运行，勿单独移动 EXE） |
| `dreampaper-*-x64-mac.dmg` | macOS Intel |
| `dreampaper-*-arm64-mac.dmg` | macOS Apple Silicon |
| `dreampaper-*-amd64.deb` | Ubuntu 22.04 及以上 x86-64（Debian 系） |

### 首次打开

未购买商业开发者证书：Windows 未做商业代码签名，macOS 使用 ad-hoc 签名、未做 Developer ID 公证，系统可能提示：

- **macOS**：双击提示「无法验证开发者」。右键点 App → 选「打开」→ 再点一次「打开」。只需操作一次。
- **Windows**：SmartScreen 提示「已保护你的电脑」。点「更多信息」→「仍要运行」。
- **Windows 便携版**依赖系统已有 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)。如果缺少，请先安装它，或改用安装版；ZIP 中的主程序、OCR 辅助程序、`ort/`、字体等必须保持完整。
- **Linux**：`sudo apt install ./dreampaper-*-amd64.deb`。包内已声明 `libwebkit2gtk-4.1-0`、`libgtk-3-0`、`libgomp1` 依赖，由 apt 解析。在 Ubuntu 22.04 x86-64 上构建，可在 22.04 及更新的发行版上安装；不提供 RPM 与 AppImage。

五类产物都随包提供 OCR 推理引擎，不内置模型。首次在设置页下载约 133 MiB 模型；检测/识别模型在 PaddlePaddle 官方 ModelScope 与 Hugging Face 镜像间自动回退，所有文件按锁定大小与 SHA-256 校验，完成后即可离线识别；无需安装 Python 或 ONNX Runtime。引擎/模型不可用时，手工文字、取色修补和裁剪仍可使用。发布流水线必须对五类最终产物执行真实下载和推理门禁，全部通过后才生成草稿，不自动公开。

桌面版的配置与产物落在系统应用数据目录，而非 `~/.dreampaper/`：

| 平台 | 路径 |
| --- | --- |
| macOS | `~/Library/Application Support/com.dreampaper.app/` |
| Windows | `%APPDATA%\com.dreampaper.app\` |
| Linux | `~/.local/share/com.dreampaper.app/`（遵循 `XDG_DATA_HOME`） |

---

## 准备 Template 库（科研图）

本仓库**不附带** PaperBananaBench。科研图模式需要本地参考图库。

1. 下载 [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench)（约 266MB）
2. 解压到仓库根目录，结构如下：

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

3. 重启后端后，科研图页即可选择 1–3 张 template

> 仅用幻灯片模式时可不下载。

**桌面版**不读仓库目录，改在「模板库」页导入：

- **导入模板包**：解压 PaperBananaBench 后，点「选择目录…」选中解压出的那个目录（内含 `diagram/` 与 `plot/`），一次导入全部
- **导入图片**：单张导入自有模板，填类型 / 分类 / 描述即可

导入后的模板复制到应用数据目录，与仓库解耦，换分支或删仓库都不影响。

---

## 本地启动

```bash
python3 -m venv .venv
source .venv/bin/activate   # Windows: .venv\Scripts\activate
pip install -r requirements.txt
npm install
```

```bash
# 终端 1 — 后端
uvicorn backend.app.main:app --reload --host 127.0.0.1 --port 8000

# 终端 2 — 前端
npm run dev
```

打开 http://127.0.0.1:5173

---

## 桌面端构建与发布

原生构建要求 Node.js 22、Rust 和对应平台开发工具（Windows 为 Visual Studio 2022 C++，macOS 为 Xcode，Linux 为 `libwebkit2gtk-4.1-dev` / `libgtk-3-dev` 等 Tauri 依赖）。ORT 构建另需 Python 3.12、CMake 4.1.2、Ninja 1.13.0，这些仅用于构建，不随应用分发。

本地桌面调试用 `npm run tauri:dev`：它会先把 OCR 辅助进程的调试构建暂存到 `src-tauri/binaries/`，并保证 `runtime/ort` 资源目录存在（Tauri 构建脚本对 `externalBin` 与资源目录都有要求）。没执行过 `gate:ort` 时磁盘上没有 ONNX Runtime，OCR 不可用而其余功能照常；要用完整引擎就先跑下面的 `gate:ort` 与 `sidecar`。

```bash
npm ci
python -m pip install cmake==4.1.2 ninja==1.13.0
npm run gate:ort
npm run sidecar
cargo test --locked --manifest-path src-tauri/Cargo.toml
npm run tauri:build
```

`gate:ort` 在当前架构从锁定提交构建 CPU 运行时，生成逐文件清单；`sidecar` 严格验证后暂存完整引擎，因此必须先于主程序测试执行。无运行时下载 URL 的平台也可在 CI 原生构建，禁止缺少引擎时降级出包。

GitHub Actions 按 Windows x64、macOS Intel / ARM、Ubuntu 22.04 x86-64 四个平台构建，并从最终 setup 安装目录、portable ZIP 解压目录、DMG 复制出的 App 以及 deb 解包目录执行门禁。手动运行只上传测试产物；`v*` tag 触发的流程在全部通过后生成一个 Release 草稿。运行时来源清单及推理报告保存在 Actions artifacts。

Linux 平台从锁定提交原生构建 ORT，并校验目标架构、构建机残留路径与动态依赖；`.deb` 由 Tauri 打包，运行依赖写在 `src-tauri/tauri.linux.conf.json` 里，发布门禁拿这份名单逐项核对最终包的控制字段（不假设上游会不会推导默认依赖）。Linux 验收在清理 `LD_LIBRARY_PATH`、`LD_PRELOAD`、`LD_AUDIT` 的环境里进行；构建基线为 Ubuntu 22.04 x86-64，产物可在 22.04 及更新的发行版上使用。

macOS 最终 DMG 由 `scripts/bundle.mjs` 封装；仅 OCR 辅助程序具有 ad-hoc 动态库加载例外，主程序保留默认 Hardened Runtime。不得以 Tauri 中间 App 代替该最终包分发。

macOS 部署目标为 13.0；现代 CI runner 的门禁通过不等同于已完成 macOS 13.0 或最低 Windows 系统的实机测试。签名与最低系统验证限制必须保留在发布说明中。

---

## 模型配置介绍

三个角色分开配置，各自独立的协议 / URL / 模型 / 密钥，在「设置」页保存后写入 `~/.dreampaper/config.json`。

1. **`design model`（规划模型，需支持多模态输入）**：负责对用户上传的资料 / 图片进行深度理解，编排目标成图的内容与布局，生成最终的制图描述。强风格一致性约束来自 template 底图。上游模型需支持图像输入，支持 OpenAI / Anthropic 协议。
2. **`implement model`（制图模型）**：接收 `design model` 的输出内容，制作最终的效果图。支持 OpenAI 的 `v1/images/generations` 和 gemini 的协议，可接入 gpt-image-2、nano-banana-2。
3. **`search model`（搜索模型）**：抽取用户输入资料 / 图片中的实物素材，如硬件实物（雷达、相机、无人机等）、软件产品（Claude Code、Codex、Pi 等）。复用现实网络素材图片，辅助 `design model` 进行规划，抑制图像编造，并丰富图像展示效果。可自定义 xAI search（配置 Grok search model）或使用 Tavily，默认 DuckDuckGo。

### 协议下拉

| 角色 | 协议 | 请求端点 | 典型配置 |
| --- | --- | --- | --- |
| design | `openai_responses`（默认） | `{URL}/v1/responses` | `https://api.openai.com` + `gpt-5.4` |
| design | `openai_chat` | `{URL}/v1/chat/completions` | 任意 OpenAI 兼容网关 |
| design | `anthropic_messages` | `{URL}/v1/messages` | `https://api.anthropic.com` |
| implement | `image2`（默认） | `{URL}/v1/images/generations`，带底图时走 `/v1/images/edits` | `https://api.openai.com` + `gpt-image-2` |
| implement | `banana2` | `{URL}/{版本}/interactions` | gemini 协议网关 + `nano-banana-2` |
| search | `duckduckgo`（默认） | DuckDuckGo 无 JS 页面 | 免密钥、免地址（两个字段都不显示），端点写死在代码里 |
| search | `tavily` | `{URL}/search` | `https://api.tavily.com`，需 API key |
| search | `grok_search` | `{URL}/v1/chat/completions` | OpenAI 兼容网关（如 grok2api）+ 自带联网能力的模型 ID（如 `grok-build-0.1`） |

> URL 只填到域名即可，未带 `/v1` 时会自动补全；`banana2` 例外，版本由「版本」字段控制（默认 `v1beta`）。

搜索角色按**协议分开存**：三个协议各有一份自己的 URL / 模型 / 密钥，在设置里切换协议就是切换到对应那份，不会把上一个协议的值带过去，也不会丢掉之前填好的（包括密钥）。表单只显示该协议真正会用到的字段：`duckduckgo` 一个字段都不用填（端点写死、免密钥，所以 URL 与密钥都不显示），`tavily` 不读模型所以不显示模型，只有 `grok_search` 三栏齐全。从旧版升上来时，如果某份档案里带着别的协议的地址与模型（旧版只有一个搜索档案，切协议会把值留在原地），它会整份归到 `grok_search`，原协议补一份干净的默认档案，因此不需要重填。

`duckduckgo` 是免密钥的那个协议，抓的是 DuckDuckGo 的无 JS 页面。它对人机验证很敏感：只带 `User-Agent` 会被判成机器人，直接回 HTTP 202 反爬页（页面里一个结果都没有）；必须带上浏览器整页跳转才有的 `Sec-Fetch-*` 与 `Upgrade-Insecure-Requests` 头，而且同一个出口 IP 请求密了也会被限。所以被拴住时不会伪装成“没有来源”：反爬页会被当成错误让任务失败，错误里点名换 `tavily` 或 `grok_search`。长期使用建议就用这两个带正式 API 的协议。

检索失败（路由错、鉴权失败、被限流、被反爬）对**所有**协议一视同仁：任务直接失败，错误写明原因，不会静默降级成“这个主体查不到”——否则用户看不出基础设施出了问题，还会拿着一份没有依据的图继续跑完。真正的“搜到 0 条”仍然照常降级（提示词里有专门的分支）。失败的任务可以直接「重试」：报文的检索会重新发，已经完成的步骤从缓存回放。

`grok_search` 就是「OpenAI 兼容的 chat completions + 要求上游模型自己调用联网工具」，[grok2api](https://github.com/chenyme/grok2api) 即为此类网关。模型名填部署里实际可用的模型 ID（例如自带联网能力的 `grok-build-0.1`），没有前缀要求，能否联网由服务端决定。请求固定非流式、设置 `tool_choice: required`，即客户端要求至少调用一种工具（服务端实际执行了什么，客户端无法断言）。工具声明始终同时包含 `x_search` 与 `web_search`：缺哪个补哪个，已配置的工具条目原样保留，不会重复追加。每次检索实际发出去的查询内容与拿回的结果都会写进任务记录，并在结果卡片的「生成过程」里按执行顺序作为一行可折叠详情展示（查询内容 / 查询结果两块），失败的调用同样记录。模型不支持这两个工具时，部署有时会回 HTTP 400 `A tool_choice was set on the request but no tools were specified`；因为客户端无法自行断言，请求失败（路由、鉴权、HTTP 错误）一律让任务失败，不降级成“没有来源”的正常结果。

### 出图参数下拉

`implement` 选 `image2` 时：

| 字段 | 可选值 | 默认 | 说明 |
| --- | --- | --- | --- |
| 尺寸 `size` | `auto` / `16:9` / `1024x1024` / `1200x675` / `928x1664` / `3000x1000` | `1200x675` | OpenAI 尺寸。支持 auto、比例字符串，或任意 宽x高 / 宽*高；尺寸会自动归到最接近比例，并按面积推导 1K/2K/4K。 |
| 质量 `quality` | `auto` / `low` / `medium` / `high` / `hd` | `auto` | 兼容字段；size 为具体尺寸时由尺寸优先推导分辨率；size 为空、auto 或比例时，medium 映射到 2K，high/hd 映射到 4K，其它默认 1K。 |
| 格式 `output_format` | `png` / `jpeg` / `webp` | `png` | 图片格式 |
| response `response_format` | `url` / `b64_json` | `url` | 响应格式，`url` 图片 URL，`b64_json` 图片 Base64 编码 |

`implement` 选 `banana2` 时：

| 字段 | 可选值 | 默认 |
| --- | --- | --- |
| 比例 `aspect_ratio` | `16:9` / `4:3` / `1:1` / `3:2` | `16:9` |
| 清晰度 `image_size` | `1K` / `2K` / `4K` | `4K` |
| 倾向 `thinking_level` | `minimal` / `high` | `high` |
| 格式 `mime_type` | `image/png` / `image/jpeg` / `image/webp` | `image/png` |
| 版本 `api_version` | 手填 | `v1beta` |

### 通用与运行时参数

| 字段 | 适用角色 | 范围 | 默认 / 建议 |
| --- | --- | --- | --- |
| 超时(秒) | 全部 | 5–1800 | design 120；implement 600–900（同步出图慢）；search 15 |
| 重试次数 | 全部 | 0–8 | design 2 / implement 3 / search 1 |
| 流式 | design | 开 / 关 | 关 |
| 结果数 | search | 1–8 | 3 |
| 代理 | 全局 | URL 或端口号 | `http://127.0.0.1:7890`，留空表示不指定代理（shell 里的 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` 不会被自动采用，代理只认这里填的值） |
| 规划并发 | 全局（幻灯片） | 1–20 | 留空 = 跟随页数 |
| 制图并发 | 全局（幻灯片） | 1–20 | 留空 = 跟随页数，建议先设 1 降低网关 502 |

### 视觉主体识别

科研图与幻灯片链路在检索实物 / logo 素材前，先让 design 模型读一遍资料，挑出其中值得检索真实外观的主体（科研图读标题与方法，幻灯片读资料文本与附件）；这是内部固定流程，不占结果卡片的阶段行，也不需要维护词表——新产品、新模型、新工具不需要谁去补词库。约束只影响版式与配色，不进入这一步，「微软雅黑」「白色底」这类写法不会被当成可检索的主体。

判定规则：模型返回的每个主体都必须是资料原文里出现过的字符串（忽略大小写、空格与标点差异：`Grok Bot` 对得上原文 `grokbot`），原文里没有的品牌或型号一律丢弃，所以模型凭记忆多报也不会污染检索；最多取 8 个。识别失败、提示词缺失或模型认为没有可检索主体时，这一步整体跳过（图照画，只是没有客观外观描述可依），任务不会因此失败。

识别提示词是 `prompts/global/visual_subjects.md`：想调整「什么样的东西值得检索」直接改这一份即可。桌面版内嵌该文件，同时与其它提示词一样随安装包打包到资源目录 `prompts/`（开发运行时取仓库根目录的 `prompts/`），外部文件优先生效。

检索阶段的输入只有文本（`grok_search` 等协议同样如此，母版截图不进入检索请求），每个主体一次查询，产出的文本描述与来源链接随提示词交给制图模型，报文记录在结果卡片里。

---

## 使用

1. **设置**：配置 Design / Implement（及可选 Search、代理、并发），保存  
2. **科研图**：选 template → 填标题与方法 → 生成  
3. **幻灯片**：上传母版图 → 填资料与页数 → 生成  

顶栏的 **Simple Mode** 只跑提示词链路：仍需要 design（与可选 search）模型，但不调用制图模型，也不要求 implement 凭据。开启后结果卡片的「生成过程」里会多出一行「最终制图提示词」：本应由制图模型收到的完整提示词——科研图一条，幻灯片全部页面按页码排列——保留换行、可选中或一键复制。开关默认关闭，对科研图与幻灯片同时生效，并跟随任务快照：重跑历史任务时用当前开关，已经开跑的任务不受切换影响；关掉后恢复现有出图流程。简单模式的任务因此永远没有成品图，历史记录的卡片改成预览这次选中的参考母版（点击可放大，悬停提示「预览：所选母版」），标题旁附一个绿色「简单模式」标签；母版删掉了就退回原来的空图标。普通模式没有成品图只说明没跑完，不会拿参考图冒充结果。历史记录页的筛选条除了类型与状态，还可以按运行模式筛选（全部 / 普通模式 / 简单模式）。

### 失败重试与停止后继续

任务失败或手动停止后，结果卡片上会出现**重试**（失败）/ **继续任务**（已停止）：点它不会从零重跑，而是用**同一条任务记录**接着上次的进度往下走——母版分析、大纲、逐页规划、已经出好的图片都不再重算，只有没跑完的那一步以后才真的花钱。

原理是任务内的答案缓存：每次真正调用模型（设计与制图）或联网检索之前，先把这次调用的全部输入（步骤名、模型、系统与用户提示词、随请求发出去的图片、制图参数）算成一个内容哈希，成功的答案按这个哈希存在任务目录里；重跑时流水线照常从头走一遍，只是命中缓存的步骤直接回放（界面上照旧出现这一步，内容也一样），网络调用被省掉。因为键就是输入本身，**改了输入、换了模型、改了提示词或换了母版都会自然变成另一次调用**，不会拿旧结果冒充新结果；失败的调用不写缓存，重跑时必须真的重试。

缓存随任务生命周期：任务成功后清空，失败或停止时保留；删除任务时连同目录一起删除。已经成功完成的任务没有可补的步骤，所以不提供继续按钮——想要再来一次用近期任务里的「重跑」（新建一条任务）。该功能目前只在桌面端。

| 路径 | 内容 |
| --- | --- |
| `~/.dreampaper/config.json` | 模型配置 |
| `~/.dreampaper/assets/` | 上传文件 |
| `~/.dreampaper/jobs/` | 任务记录与出图 |

### 工作台（桌面版）

左侧导航"历史数据"下方的**工作台**用于修复生成图中的乱码或错字：从生成结果、历史记录或本地文件打开图片，框选出错区域后自动用底色覆盖并识别文字，得到可编辑的文本层；随后可调整文案与排版、按原图整数像素裁剪，并导出与源图同分辨率的无损 PNG。源图始终只读，每张图的修改保存在独立工程中，可随时恢复。

- **文字识别（OCR）完全离线**：识别引擎（纯 Rust 辅助进程 + ONNX Runtime）随安装包内置；首次使用时按提示下载 PP-OCRv6 medium 检测/识别模型与文字方向分类模型（约 133 MiB，来源与校验值锁定在应用内），之后图片与识别结果都不会离开本机。设置页可查看引擎版本、更新或删除模型。
- 未安装模型时，纯色修补、裁剪与手工添加文字仍可使用。
- 工作台数据位于应用数据目录的 `workbench/`（工程与源图快照）和 `ocr/`（模型）；设置页提供占用统计与只清理无引用快照的安全清理。

---

## 致谢

科研图 template 来自 [PaperBananaBench](https://huggingface.co/datasets/dwzhu/PaperBananaBench)（[PaperBanana](https://github.com/dwzhu-pku/PaperBanana)）。

工作台文字识别使用 [PaddleOCR](https://github.com/PaddlePaddle/PaddleOCR) 的 PP-OCRv6 模型与 [RapidOCR](https://github.com/RapidAI/RapidOCR) 分发的文字方向分类模型（Apache-2.0），推理运行时为 [ONNX Runtime](https://onnxruntime.ai/)（MIT）；Rust 侧预/后处理移植自 [ppocr-rs](https://crates.io/crates/ppocr-rs)（Apache-2.0）。回退字体 Noto Sans SC 采用 SIL OFL 1.1。详见 `src-tauri/ocr/THIRD_PARTY.md`。

---

## Star History

<a href="https://www.star-history.com/?repos=dream-rec%2Fdreampaper&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=dream-rec/dreampaper&type=date&theme=dark&legend=top-left&sealed_token=ej8Oq3_JvVb6NVk_lL4pi27YYDnqlXQLdD8BpyOsbkS5_HhFb2FGeY4umbQzYpSbSiW47djwIZfrKUTq5S7UnjvVjk9qSjHwZ4_Yko3LxQnqER4FuqXv5A" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=dream-rec/dreampaper&type=date&legend=top-left&sealed_token=ej8Oq3_JvVb6NVk_lL4pi27YYDnqlXQLdD8BpyOsbkS5_HhFb2FGeY4umbQzYpSbSiW47djwIZfrKUTq5S7UnjvVjk9qSjHwZ4_Yko3LxQnqER4FuqXv5A" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=dream-rec/dreampaper&type=date&legend=top-left&sealed_token=ej8Oq3_JvVb6NVk_lL4pi27YYDnqlXQLdD8BpyOsbkS5_HhFb2FGeY4umbQzYpSbSiW47djwIZfrKUTq5S7UnjvVjk9qSjHwZ4_Yko3LxQnqER4FuqXv5A" />
 </picture>
</a>

---
## 开源协议

本项目采用 [PolyForm Noncommercial License 1.0.0](LICENSE)，**允许非商业使用，禁止商业使用**。

| 用途 | 是否允许 |
| --- | --- |
| 个人学习、研究、实验、业余项目 | ✅ |
| 阅读、修改源码，二次开发与分发 | ✅（需附带本协议） |
| 公司内部生产使用、对外提供付费或商业服务 | ❌ |
| 将本项目或其衍生版本作为商品出售 | ❌ |
