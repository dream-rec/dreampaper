import { createHash } from 'node:crypto';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, posix, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const locked = JSON.parse(readFileSync(join(root, 'src-tauri/runtime/runtime.json'), 'utf8'));
export const targets = {
  'x86_64-pc-windows-msvc': { platform: 'win32', arch: 'x64', library: 'onnxruntime.dll', suffix: '.exe' },
  'x86_64-apple-darwin': { platform: 'darwin', arch: 'x64', library: 'libonnxruntime.dylib', suffix: '' },
  'aarch64-apple-darwin': { platform: 'darwin', arch: 'arm64', library: 'libonnxruntime.dylib', suffix: '' },
  // 在 Ubuntu 22.04 上构建，glibc 能在 22.04 和更新的发行版上加载。只发 x86-64。
  'x86_64-unknown-linux-gnu': { platform: 'linux', arch: 'x64', library: 'libonnxruntime.so', suffix: '', debian_arch: 'amd64' }
};

export function targetInfo(triple) {
  const info = targets[triple];
  if (!info) throw new Error(`不支持的目标：${triple}`);
  return info;
}

export function digest(path) {
  const bytes = readFileSync(path);
  return { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') };
}

export function filesIn(directory, prefix = '') {
  return readdirSync(join(directory, prefix), { withFileTypes: true }).flatMap((entry) => {
    const name = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`运行时目录不允许符号链接：${name}`);
    return entry.isDirectory() ? filesIn(directory, name) : [name];
  }).sort();
}

/** `dpkg-deb --field` 的输出 → 字段表。 */
export function debControlFields(output) {
  return Object.fromEntries(
    output.split('\n').map((line) => line.trim()).filter(Boolean)
      .map((line) => [line.slice(0, line.indexOf(':')), line.slice(line.indexOf(':') + 1).trim()])
  );
}

/**
 * deb 控制字段里的包名。
 *
 * 锁定 CLI（tauri-cli 2.11.4，`crates/tauri-bundler/src/bundle/linux/debian.rs`）
 * 用 `heck::AsKebabCase(product_name)`：`DreamPaper` → `dream-paper`。
 * 二进制名 `dreampaper` 与资源目录 `/usr/lib/DreamPaper` 不走这一套，不能混用。
 */
export function debPackageName(productName) {
  return productName
    .replace(/([a-z0-9])([A-Z])/g, '$1-$2')
    .replace(/[^A-Za-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .toLowerCase();
}

/**
 * deb 控制字段与发布约定的差异（Linux 专用，路径分隔符固定为 /）。
 *
 * 依赖取配置里声明的名单：上游会不会自己推导默认依赖不在这里假设。
 */
export function debControlProblems(control, expected) {
  const problems = [];
  if (control.Version !== expected.version) problems.push(`deb 版本不符：${control.Version}`);
  if (control.Architecture !== expected.arch) problems.push(`deb 架构不符：${control.Architecture}`);
  if (control.Package !== expected.packageName) problems.push(`deb 包名不符：${control.Package}（期望 ${expected.packageName}）`);
  const declared = (control.Depends || '').split(',').map((item) => item.trim().split(/\s+/)[0]).filter(Boolean);
  for (const required of expected.depends) {
    if (!declared.includes(required)) problems.push(`deb 未声明运行依赖 ${required}：${control.Depends}`);
  }
  return problems;
}

const SYSTEM_LIBRARY_DIRS = ['/lib', '/lib64', '/usr/lib', '/usr/lib64'];
// 只认完整的 $ORIGIN / ${ORIGIN} 起始标记。
const ORIGIN = /^\$(?:ORIGIN|\{ORIGIN\})/;

/**
 * ELF 的 RPATH/RUNPATH 与 ldd 结果只能指向系统库目录或包内（Linux 专用）。
 *
 * 一律先规范化再做目录边界判断，避免 `/lib/../home/...` 这类前缀绕行；
 * 包内路径还要过一遍实际路径解析（默认原样返回，探针传入真实 realpath），
 * 否则一个指向包外的符号链接就能蒙过归属检查。
 */
export function linuxBinaryProblems({ file, dynamic = '', linked = '', root, realpath = (path) => path }) {
  const problems = [];
  const rootPath = posix.normalize(root);
  const inside = (path) => path === rootPath || path.startsWith(`${rootPath}/`);
  const checkDependency = (path) => {
    const normalized = posix.normalize(path);
    if (SYSTEM_LIBRARY_DIRS.some((dir) => normalized === dir || normalized.startsWith(`${dir}/`))) return;
    if (!inside(normalized)) {
      problems.push(`依赖不在系统库目录也不在包内：${path}（${file}）`);
      return;
    }
    const real = posix.normalize(realpath(normalized));
    if (!inside(real)) problems.push(`依赖的符号链接指向包外：${path} → ${real}（${file}）`);
  };

  for (const match of dynamic.matchAll(/\((?:RPATH|RUNPATH)\)[^\n]*\[([^\]]+)\]/g)) {
    for (const entry of match[1].split(':')) {
      if (!entry) continue;
      const marker = entry.match(ORIGIN);
      // $ORIGIN 之后必须结束或接目录分隔符：$ORIGINBAD、ORIGIN/… 都不算。
      if (!marker || (entry.length > marker[0].length && entry[marker[0].length] !== '/')) {
        problems.push(`包内残留构建机路径：${entry}（${file}）`);
        continue;
      }
      const target = posix.normalize(posix.join(posix.dirname(file), entry.slice(marker[0].length)));
      if (!inside(target)) problems.push(`包内 RPATH 指向包外：${entry}（${file}）`);
    }
  }

  if (/not found/i.test(linked)) problems.push(`缺少动态依赖：${file}\n${linked}`);
  for (const line of linked.split('\n')) {
    const trimmed = line.trim();
    if (!trimmed) continue;
    const arrow = trimmed.indexOf('=>');
    // 无 => 的绝对路径行（如 ld-linux 本身）也要判归属，不能默默跳过。
    const target = (arrow >= 0 ? trimmed.slice(arrow + 2) : trimmed).trim().split(' (')[0];
    if (target === 'not found') continue;
    if (!target.startsWith('/')) {
      if (arrow >= 0) problems.push(`依赖解析为非绝对路径：${target}（${file}）`);
      continue;
    }
    checkDependency(target);
  }
  return problems;
}

export function verifyRuntime(directory, triple) {
  const info = targetInfo(triple);
  const manifest = JSON.parse(readFileSync(join(directory, 'manifest.json'), 'utf8'));
  if (manifest.triple !== triple || manifest.commit !== locked.commit || manifest.version !== locked.version) {
    throw new Error('运行时来源或目标与锁定清单不符');
  }
  if (manifest.recipe_sha256 !== digest(join(root, 'scripts/ort.mjs')).sha256) {
    throw new Error('运行时构建配方已变化，请重新构建');
  }
  if (!Array.isArray(manifest.files) || !manifest.files.some((file) => file.name === info.library)) {
    throw new Error(`运行时清单缺少 ${info.library}`);
  }
  const names = new Set();
  for (const file of manifest.files) {
    if (typeof file.name !== 'string' || !/^[\w.-]+(?:\/[\w.-]+)*$/.test(file.name) || file.name.split('/').includes('..') || names.has(file.name)) {
      throw new Error('运行时清单包含无效或重复路径');
    }
    names.add(file.name);
    if (!Number.isSafeInteger(file.bytes) || file.bytes <= 0 || !/^[a-f0-9]{64}$/.test(file.sha256)) {
      throw new Error(`运行时摘要无效：${file.name}`);
    }
    const path = join(directory, file.name);
    if (!statSync(path).isFile()) throw new Error(`运行时文件不存在：${file.name}`);
    const actual = digest(path);
    if (actual.bytes !== file.bytes || actual.sha256 !== file.sha256) throw new Error(`运行时校验失败：${file.name}`);
  }
  const extra = filesIn(directory).filter((name) => name !== 'manifest.json' && !names.has(name));
  if (extra.length) throw new Error(`运行时存在未登记文件：${extra.join(', ')}`);
  return manifest;
}
