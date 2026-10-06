import { cpSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { root, targetInfo, verifyRuntime } from './runtime.mjs';

const [triple, version] = process.argv.slice(2);
const info = targetInfo(triple);
if (!/^\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(version || '')) throw new Error('版本号无效');
const binary = resolve(root, 'src-tauri/target', triple, 'release');
const output = resolve(root, 'release-artifacts');
mkdirSync(output, { recursive: true });
function take(directory, match) {
  const files = readdirSync(directory).filter(match);
  if (files.length !== 1) throw new Error(`${directory} 中产物必须唯一：${files}`);
  return join(directory, files[0]);
}
if (info.platform === 'win32') {
  const setup = take(join(binary, 'bundle/nsis'), (name) => name.endsWith('-setup.exe'));
  copyFileSync(setup, join(output, `dreampaper-${version}-setup.exe`));
  verifyRuntime(join(root, 'src-tauri/runtime', triple), triple);
  const temporary = resolve(root, 'src-tauri/target/portable');
  const directory = join(temporary, `dreampaper-${version}-portable`);
  rmSync(temporary, { recursive: true, force: true });
  mkdirSync(directory, { recursive: true });
  try {
    copyFileSync(join(binary, 'dreampaper.exe'), join(directory, 'dreampaper.exe'));
    copyFileSync(join(root, `src-tauri/binaries/dreampaper-ocr-${triple}.exe`), join(directory, 'dreampaper-ocr.exe'));
    cpSync(join(root, 'src-tauri/runtime/ort'), join(directory, 'ort'), { recursive: true });
    cpSync(join(root, 'src-tauri/fonts'), join(directory, 'fonts'), { recursive: true });
    cpSync(join(root, 'prompts'), join(directory, 'prompts'), { recursive: true });
    copyFileSync(join(root, 'LICENSE'), join(directory, 'LICENSE'));
    writeFileSync(join(directory, '使用说明.txt'), '解压整个目录后运行 dreampaper.exe，请勿单独移动 EXE。\r\n需要系统已安装 WebView2 Runtime。OCR 引擎随包提供，模型在设置页首次下载，完成后可离线识别。\r\n应用数据仍存放在系统用户数据目录。\r\n');
    const archive = join(output, `dreampaper-${version}-portable.zip`);
    rmSync(archive, { force: true });
    const result = spawnSync('powershell.exe', ['-NoProfile', '-NonInteractive', '-Command', 'Compress-Archive -LiteralPath $env:DP_SOURCE -DestinationPath $env:DP_ARCHIVE -CompressionLevel Optimal'], {
      stdio: 'inherit', env: { ...process.env, DP_SOURCE: directory, DP_ARCHIVE: archive }
    });
    if (result.error) throw result.error;
    if (result.status !== 0 || !existsSync(archive)) throw new Error('便携 ZIP 创建失败');
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
} else if (info.platform === 'linux') {
  // deb 由 Tauri 直接产出（资源、sidecar 与 ORT 都在包内），这里只做归集命名。
  const deb = take(join(binary, 'bundle/deb'), (name) => name.endsWith('.deb'));
  copyFileSync(deb, join(output, `dreampaper-${version}-${info.debian_arch}.deb`));
  verifyRuntime(join(root, 'src-tauri/runtime', triple), triple);
} else {
  if (process.env.APPLE_SIGNING_IDENTITY && process.env.APPLE_SIGNING_IDENTITY !== '-') throw new Error('当前打包流程仅支持 ad-hoc 签名');
  const source = take(join(binary, 'bundle/macos'), (name) => name.endsWith('.app'));
  const temporary = mkdtempSync(join(binary, 'dmg-'));
  const app = join(temporary, 'DreamPaper.app');
  try {
    command('ditto', [source, app]);
    command('codesign', ['--force', '--sign', '-', '--options', 'runtime', '--entitlements', join(root, 'src-tauri/ocr/entitlements.plist'), join(app, 'Contents/MacOS/dreampaper-ocr')]);
    command('codesign', ['--force', '--sign', '-', '--options', 'runtime', app]);
    command('codesign', ['--verify', '--deep', '--strict', app]);
    symlinkSync('/Applications', join(temporary, 'Applications'));
    const archive = join(output, `dreampaper-${version}-${info.arch === 'x64' ? 'x64' : 'arm64'}-mac.dmg`);
    command('hdiutil', ['create', '-volname', 'DreamPaper', '-srcfolder', temporary, '-format', 'UDZO', '-ov', archive]);
  } finally {
    rmSync(temporary, { recursive: true, force: true });
  }
}
function command(program, args) {
  const result = spawnSync(program, args, { stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${program} 退出码 ${result.status}`);
}
console.log(readdirSync(output).join('\n'));
