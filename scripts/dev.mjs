import { copyFileSync, mkdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { root, targetInfo } from './runtime.mjs';

// `tauri dev` 的构建脚本要求 externalBin 与 runtime/ort 资源目录存在，但这两样平时都是
// CI 产物（scripts/sidecar.mjs 生成）。开发时把 OCR 的调试构建复制过去即可：辅助进程
// 能跑，ONNX Runtime 留占位（没跑过 gate:ort 时 OCR 会报缺少引擎）。正式打包路径不变，
// 仍然必须由 scripts/sidecar.mjs 校验运行时后才能出包。
const host = execFileSync('rustc', ['--print', 'host-tuple'], { encoding: 'utf8' }).trim();
const info = targetInfo(host);
const result = spawnSync('cargo', [
  'build', '--locked', '--manifest-path', resolve(root, 'src-tauri/ocr/Cargo.toml'),
  '--bin', 'dreampaper-ocr'
], { stdio: 'inherit' });
if (result.error) throw result.error;
if (result.status !== 0) throw new Error(`OCR 辅助进程构建失败：${result.status}`);

const binaries = resolve(root, 'src-tauri/binaries');
mkdirSync(binaries, { recursive: true });
const sidecar = join(binaries, `dreampaper-ocr-${host}${info.suffix}`);
copyFileSync(resolve(root, 'src-tauri/ocr/target/debug', `dreampaper-ocr${info.suffix}`), sidecar);
mkdirSync(resolve(root, 'src-tauri/runtime/ort'), { recursive: true });
console.log(`DEV_ASSETS_OK ${sidecar}`);
