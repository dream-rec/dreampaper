use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use crate::core::workbench::{
    geom::PixelRect,
    ocr::{self, OcrPackages},
    sidecar::OcrSidecar,
    source::{DecodedSource, PngMeta},
    text::{Fonts, TextSpec, BUNDLED_FAMILY, BUNDLED_FILE},
};
use crate::error::{AppError, AppResult};
use serde_json::{json, Value};

pub(crate) fn run_if_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_none_or(|arg| arg != "--ocr-probe") {
        return None;
    }
    if args.len() != 4 {
        eprintln!("用法：--ocr-probe <prepare|cancel|install|offline|remove> <隔离目录>");
        return Some(2);
    }
    let mode = args[2].to_string_lossy();
    if !["prepare", "cancel", "install", "offline", "remove"].contains(&mode.as_ref()) {
        return Some(2);
    }
    let directory = PathBuf::from(&args[3]);
    let result = run(&mode, &directory);
    let (code, report) = match result {
        Ok(value) => (
            0,
            json!({"ok": true, "mode": mode, "version": env!("CARGO_PKG_VERSION"), "result": value}),
        ),
        Err(error) => (1, json!({"ok": false, "mode": mode, "error": error})),
    };
    if let Err(error) = std::fs::write(
        directory.join(format!("{mode}.json")),
        serde_json::to_vec_pretty(&report).unwrap(),
    ) {
        eprintln!("无法保存门禁报告：{error}");
        return Some(1);
    }
    println!("{report}");
    Some(code)
}

fn require(condition: bool, message: &str) -> AppResult<()> {
    if condition {
        Ok(())
    } else {
        Err(AppError::new("ocr_probe_failed", message))
    }
}

fn run(mode: &str, directory: &Path) -> AppResult<Value> {
    if mode == "prepare" {
        require(directory.is_dir(), "隔离目录不存在")?;
        require(
            !directory.join("data").exists() && !directory.join("probe.lock").exists(),
            "门禁必须使用全新数据目录",
        )?;
        std::fs::write(
            directory.join("probe.lock"),
            "DreamPaper OCR production probe\n",
        )?;
        std::fs::create_dir(directory.join("data"))?;
    }
    require(
        std::fs::read_to_string(directory.join("probe.lock"))?
            == "DreamPaper OCR production probe\n",
        "不是门禁专用目录",
    )?;
    let executable = std::env::current_exe()?.canonicalize()?;
    let bin = executable
        .parent()
        .ok_or_else(|| AppError::new("ocr_probe_failed", "可执行文件路径无效"))?;
    let resources = if cfg!(target_os = "macos") {
        bin.join("../Resources")
    } else if cfg!(target_os = "linux") {
        // deb 布局：Tauri 把资源装在 /usr/lib/<productName>（tauri.conf.json 的
        // "DreamPaper"），可执行文件与 sidecar 在 /usr/bin。名字写死是有意的：
        // 门禁要断言包内布局，而不是跟着当前配置走。
        bin.join("../lib/DreamPaper")
    } else {
        bin.to_path_buf()
    }
    .canonicalize()?;
    let location =
        crate::state::locate_sidecar(Some(executable.clone()), Some(resources.clone()), false);
    let bundle = if cfg!(target_os = "macos") || cfg!(target_os = "linux") {
        // macOS 的根是 .app 内的 Contents，deb 的根是 /usr（/usr/bin 与 /usr/lib）。
        bin.join("..").canonicalize()?
    } else {
        bin.to_path_buf()
    };
    for path in [&location.program, &location.runtime] {
        require(
            path.as_ref()
                .is_some_and(|p| p.canonicalize().is_ok_and(|p| p.starts_with(&bundle))),
            "引擎未完全来自最终产物",
        )?;
    }
    require(
        resources.join("fonts").join(BUNDLED_FILE).is_file(),
        "最终包缺少内置字体",
    )?;
    let sidecar = OcrSidecar::new(location);
    let packages = Arc::new(OcrPackages::default());
    let data = directory.join("data");
    match mode {
        "prepare" => {
            require(!packages.status(&data).installed, "模型被预装")?;
            let source = fixture(&resources)?;
            image::save_buffer(
                directory.join("sample.png"),
                &source.rgb,
                source.width,
                source.height,
                image::ColorType::Rgb8,
            )
            .map_err(|e| AppError::new("ocr_probe_failed", e.to_string()))?;
            Ok(json!({"engine": sidecar.status(), "installed": false, "fixture": "sample.png"}))
        }
        "cancel" | "install" => {
            require(!packages.status(&data).installed, "预期模型尚未安装")?;
            let cancelling = mode == "cancel";
            let cancelled = Arc::new(AtomicBool::new(false));
            let fired = Arc::clone(&cancelled);
            let control = Arc::clone(&packages);
            let proxy = std::env::var("HTTPS_PROXY").ok();
            packages.start_install(data.clone(), proxy, move |progress| {
                if cancelling
                    && progress.state == "downloading"
                    && progress.received > 0
                    && !fired.swap(true, Ordering::SeqCst)
                {
                    let _ = control.cancel_install();
                }
            })?;
            let started = Instant::now();
            let status = loop {
                let status = packages.status(&data);
                if !status.downloading
                    && status.progress.as_ref().is_some_and(|p| {
                        matches!(p.state.as_str(), "done" | "cancelled" | "failed")
                    })
                {
                    break status;
                }
                require(
                    started.elapsed() < Duration::from_secs(1200),
                    "模型安装超时",
                )?;
                std::thread::sleep(Duration::from_millis(100));
            };
            if cancelling {
                require(
                    cancelled.load(Ordering::SeqCst)
                        && !status.installed
                        && status
                            .progress
                            .as_ref()
                            .is_some_and(|p| p.state == "cancelled"),
                    &format!(
                        "取消下载门禁失败，已发出取消={}，状态={status:?}",
                        cancelled.load(Ordering::SeqCst)
                    ),
                )?;
                return Ok(json!({"cancelled": true, "installed": false}));
            }
            require(status.installed, &format!("安装失败：{:?}", status.error))?;
            ocr::verify_installed(&data)?;
            recognize(&sidecar, &packages, &data, directory)
        }
        "offline" => {
            // 门禁用一个立刻断开的本地代理模拟断网，写在环境变量里。
            // 正式请求不读这些变量（见 build_client），所以这次连通性检查必须
            // 把探针注入的代理显式传进去，否则会直连 example.com，误判为没断网。
            let proxy = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"]
                .into_iter()
                .find_map(|key| std::env::var(key).ok())
                .filter(|value| !value.trim().is_empty());
            let runtime = tokio::runtime::Runtime::new()?;
            let blocked = runtime.block_on(async {
                crate::core::net::build_client(5, proxy.as_deref())?
                    .get("https://example.com")
                    .send()
                    .await
                    .map(|_| false)
                    .or(Ok::<bool, AppError>(true))
            })?;
            require(blocked, "离线门禁未阻断网络")?;
            ocr::verify_installed(&data)?;
            let mut result = recognize(&sidecar, &packages, &data, directory)?;
            result["network_blocked"] = json!(true);
            Ok(result)
        }
        "remove" => {
            sidecar.shutdown();
            packages.remove(&data)?;
            require(
                !packages.status(&data).installed && packages.model_paths(&data).is_err(),
                "模型删除未生效",
            )?;
            Ok(json!({"installed": false}))
        }
        _ => unreachable!(),
    }
}

fn recognize(
    sidecar: &OcrSidecar,
    packages: &OcrPackages,
    data: &Path,
    directory: &Path,
) -> AppResult<Value> {
    let source = crate::core::workbench::source::decode_file(&directory.join("sample.png"))?;
    let result = sidecar.recognize(
        &packages.model_paths(data)?,
        &source,
        PixelRect::new(0, 0, source.width.into(), source.height.into()),
        [255; 3],
        1,
    );
    sidecar.shutdown();
    let result = result?;
    let text: String = result
        .items
        .iter()
        .map(|i| i.text.as_str())
        .collect::<String>()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    require(
        text.contains("科研绘图") && text.contains("DreamPaper"),
        &format!("中英文识别未匹配固定样图：{text}"),
    )?;
    require(
        !result.items.is_empty()
            && result.items.iter().all(|item| {
                item.score.is_finite()
                    && item.score >= 0.8
                    && item.poly.len() == 4
                    && item.poly.iter().all(|p| {
                        p.x >= 0
                            && p.y >= 0
                            && p.x <= i64::from(source.width)
                            && p.y <= i64::from(source.height)
                    })
                    && item.poly.iter().map(|p| p.x).min() < item.poly.iter().map(|p| p.x).max()
                    && item.poly.iter().map(|p| p.y).min() < item.poly.iter().map(|p| p.y).max()
            }),
        "OCR 返回无效坐标或低置信度结果",
    )?;
    Ok(json!({"engine": sidecar.status(), "recognition": result}))
}

fn fixture(resources: &Path) -> AppResult<DecodedSource> {
    let mut fonts = Fonts::load(&resources.join("fonts"));
    require(fonts.bundled_available(), "缺少固定样图字体")?;
    let spec = TextSpec {
        text: "科研绘图\nDreamPaper".into(),
        width: 650.0,
        height: 160.0,
        family: BUNDLED_FAMILY.into(),
        size: 48.0,
        weight: 400,
        italic: false,
        line_height: 1.4,
        letter_spacing: 0.0,
        align: "left".into(),
        valign: "top".into(),
        auto_fit: false,
    };
    let (pixels, _, _, _) = fonts.rasterize(&spec, [0; 3])?;
    let rgb = pixels
        .data()
        .chunks_exact(4)
        .flat_map(|p| {
            [
                p[0].saturating_add(255 - p[3]),
                p[1].saturating_add(255 - p[3]),
                p[2].saturating_add(255 - p[3]),
            ]
        })
        .collect();
    Ok(DecodedSource {
        width: pixels.width(),
        height: pixels.height(),
        rgb,
        had_alpha: false,
        icc: None,
        png: PngMeta::default(),
        jfif_dpi: None,
    })
}
