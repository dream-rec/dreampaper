import { spawnSync } from 'node:child_process';
import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { debControlFields, debControlProblems, debPackageName, digest, filesIn, linuxBinaryProblems, locked, root, targetInfo, verifyRuntime } from './runtime.mjs';

function fixture(run) {
  const directory = mkdtempSync(join(tmpdir(), 'runtime 空格 '));
  try {
    mkdirSync(join(directory, 'licenses'));
    writeFileSync(join(directory, 'onnxruntime.dll'), 'test-runtime');
    writeFileSync(join(directory, 'licenses/LICENSE'), 'test-license');
    const manifest = {
      triple: 'x86_64-pc-windows-msvc', commit: locked.commit, version: locked.version,
      recipe_sha256: digest(join(root, 'scripts/ort.mjs')).sha256,
      files: filesIn(directory).map((name) => ({ name, ...digest(join(directory, name)) }))
    };
    const save = () => writeFileSync(join(directory, 'manifest.json'), JSON.stringify(manifest));
    save(); run(directory, manifest, save);
  } finally { rmSync(directory, { recursive: true, force: true }); }
}

test('文件 URL 正确解码 Windows 盘符、空格和中文', () => {
  assert.equal(fileURLToPath('file:///D:/a/%E4%B8%AD%E6%96%87%20repo/scripts/', { windows: true }), 'D:\\a\\中文 repo\\scripts\\');
  assert.equal(targetInfo('aarch64-apple-darwin').arch, 'arm64');
  assert.equal(targetInfo('x86_64-pc-windows-msvc').suffix, '.exe');
  assert.throws(() => targetInfo('unsupported'));
});
test('完整运行时清单可以验证', () => fixture((directory) => {
  assert.equal(verifyRuntime(directory, 'x86_64-pc-windows-msvc').commit, locked.commit);
}));
test('缺库、篡改、额外依赖均阻止打包', () => fixture((directory) => {
  writeFileSync(join(directory, 'extra.dll'), 'extra');
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /未登记/);
  rmSync(join(directory, 'extra.dll'));
  writeFileSync(join(directory, 'onnxruntime.dll'), 'test-runtimX');
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /校验失败/);
  rmSync(join(directory, 'onnxruntime.dll'));
  assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
}));
test('空摘要、错误源码、错误架构或配方均失败', () => {
  for (const change of [
    (m) => { m.files[0].sha256 = ''; },
    (m) => { m.commit = 'incorrect'; },
    (m) => { m.triple = 'aarch64-apple-darwin'; },
    (m) => { m.recipe_sha256 = '0'.repeat(64); }
  ]) fixture((directory, manifest, save) => {
    change(manifest); save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
  });
});
test('拒绝路径穿越和重复清单项', () => {
  for (const name of ['../secret', '/absolute', 'C:/secret', 'licenses/../secret']) fixture((directory, manifest, save) => {
    manifest.files[0].name = name; save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'));
  });
  fixture((directory, manifest, save) => {
    manifest.files.push(manifest.files[0]); save(); assert.throws(() => verifyRuntime(directory, 'x86_64-pc-windows-msvc'), /重复/);
  });
});
test('正式流程禁止缺引擎逃生口，测试必须晚于引擎暂存', () => {
  const workflow = readFileSync(join(root, '.github/workflows/release.yml'), 'utf8');
  assert.ok(!workflow.includes('DREAMPAPER_ALLOW_MISSING_RUNTIME'));
  assert.ok(workflow.indexOf('scripts/sidecar.mjs') < workflow.indexOf('cargo test'));
  assert.ok(workflow.includes('needs: build'));
  assert.ok(workflow.indexOf('scripts/probe.mjs') < workflow.indexOf('name: 上传已验收产物'));
});

test('macOS 动态库加载例外仅限辅助程序，最终包重新签名', () => {
  const bundle = readFileSync(join(root, 'scripts/bundle.mjs'), 'utf8');
  assert.ok(bundle.includes("join(app, 'Contents/MacOS/dreampaper-ocr')"));
  assert.ok(bundle.includes("['--force', '--sign', '-', '--options', 'runtime', app]"));
  assert.ok(bundle.includes("command('hdiutil', ['create'"));
  const config = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.macos.conf.json'), 'utf8'));
  assert.ok(!config.bundle.macOS.entitlements);
  assert.notEqual(config.bundle.macOS.hardenedRuntime, false);
});

test('引擎构建仅编译生产动态库，项目和最终包测试仍执行', () => {
  const ort = readFileSync(join(root, 'scripts/ort.mjs'), 'utf8');
  assert.ok(ort.includes("'--targets', 'onnxruntime'"));
  assert.ok(ort.includes('onnxruntime_BUILD_UNIT_TESTS=OFF'));
  const workflow = readFileSync(join(root, '.github/workflows/release.yml'), 'utf8');
  assert.ok(workflow.includes('cargo test --locked --manifest-path src-tauri/Cargo.toml'));
  assert.ok(workflow.includes('cargo test --locked --manifest-path src-tauri/ocr/Cargo.toml'));
  assert.ok(workflow.includes('scripts/probe.mjs'));
});

test('Linux 目标映射与 deb 归集命名一致', () => {
  const info = targetInfo('x86_64-unknown-linux-gnu');
  assert.equal(info.platform, 'linux');
  assert.equal(info.library, 'libonnxruntime.so');
  assert.equal(info.suffix, '');
  assert.equal(info.debian_arch, 'amd64');
  const bundle = readFileSync(join(root, 'scripts/bundle.mjs'), 'utf8');
  assert.ok(bundle.includes("take(join(binary, 'bundle/deb')"));
  assert.ok(bundle.includes('dreampaper-${version}-${info.debian_arch}.deb'));
  const draft = readFileSync(join(root, 'scripts/draft.mjs'), 'utf8');
  assert.ok(draft.includes("'amd64.deb'"));
  assert.ok(draft.includes('libwebkit2gtk-4.1'));
});

test('Linux 产物必须声明运行依赖并打进引擎资源', () => {
  const config = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.linux.conf.json'), 'utf8'));
  assert.deepEqual(config.bundle.targets, ['deb']);
  assert.equal(config.bundle.resources['runtime/ort/'], 'ort/');
  const depends = config.bundle.linux.deb.depends;
  assert.ok(depends.includes('libwebkit2gtk-4.1-0'));
  assert.ok(depends.includes('libgtk-3-0'));
  assert.ok(depends.includes('libgomp1'));
});

test('ELF 构建与验包都拒绝构建机路径', () => {
  const ort = readFileSync(join(root, 'scripts/ort.mjs'), 'utf8');
  assert.ok(ort.includes('readelf'));
  assert.ok(ort.includes('ldd'));
  // Apple 的部署目标只能在 darwin 上设置。
  assert.ok(ort.includes('darwin ? { MACOSX_DEPLOYMENT_TARGET'));
});

const CONTROL = debControlFields(
  'Package: dream-paper\nVersion: 0.2.1\nArchitecture: amd64\nDepends: libwebkit2gtk-4.1-0 (>= 2.38), libgtk-3-0, libgomp1\n'
);
const EXPECTED = {
  version: '0.2.1', arch: 'amd64', packageName: debPackageName('DreamPaper'),
  depends: ['libwebkit2gtk-4.1-0', 'libgtk-3-0', 'libgomp1']
};

test('deb 包名按锁定 CLI 的 kebab-case 规则推导', () => {
  // tauri-cli 2.11.4 的 debian.rs 用 heck::AsKebabCase(product_name) 写 Package 字段。
  assert.equal(EXPECTED.packageName, 'dream-paper');
  assert.equal(debPackageName('Dream Paper'), 'dream-paper');
  assert.equal(debPackageName('DreamPaper'), 'dream-paper');
  const config = JSON.parse(readFileSync(join(root, 'src-tauri/tauri.conf.json'), 'utf8'));
  assert.equal(debPackageName(config.productName), 'dream-paper');
});

test('deb 控制字段逐项核对：版本、架构、包名与声明的依赖', () => {
  assert.equal(CONTROL.Package, 'dream-paper');
  assert.deepEqual(debControlProblems(CONTROL, EXPECTED), []);
  // 带版本约束的依赖算已声明，额外依赖不算错。
  assert.deepEqual(debControlProblems({ ...CONTROL, Depends: 'libgtk-3-0 (>= 3.24), libgomp1, libwebkit2gtk-4.1-0, zlib1g' }, EXPECTED), []);
  for (const change of [
    { Version: '0.2.0' },
    { Architecture: 'arm64' },
    // 二进制名（dreampaper）与资源目录（DreamPaper）都不是包名。
    { Package: 'dreampaper' },
    { Package: 'DreamPaper' },
    { Depends: 'libgtk-3-0, libgomp1' }
  ]) assert.ok(debControlProblems({ ...CONTROL, ...change }, EXPECTED).length > 0, JSON.stringify(change));
});

test('运行依赖只允许系统库目录或包内，含 $ORIGIN 归属复核', () => {
  const file = '/pkg/usr/bin/dreampaper';
  const linked = [
    '\tlinux-vdso.so.1 (0x1)',
    '\tlibc.so.6 => /lib/x86_64-linux-gnu/libc.so.6 (0x2)',
    '\t/lib64/ld-linux-x86-64.so.2 (0x3)',
    '\tlibonnxruntime.so => /pkg/usr/lib/DreamPaper/ort/libonnxruntime.so (0x4)'
  ].join('\n');
  assert.deepEqual(linuxBinaryProblems({ file, linked, root: '/pkg' }), []);
  assert.deepEqual(linuxBinaryProblems({ file, dynamic: ' (RUNPATH) Library runpath: [$ORIGIN/../lib]', root: '/pkg' }), []);
  // 冒号分隔的多段路径与新式 ${ORIGIN} 同样能解析
  assert.deepEqual(linuxBinaryProblems({ file, dynamic: ' (RUNPATH) Library runpath: [${ORIGIN}/../lib:$ORIGIN/.]', root: '/pkg' }), []);
  // 包内但软链到包外：只靠字符串前缀看不出来。
  assert.deepEqual(
    linuxBinaryProblems({
      file, linked: '\tlibonnxruntime.so => /pkg/usr/lib/DreamPaper/ort/libonnxruntime.so (0x4)', root: '/pkg',
      realpath: (path) => (path.endsWith('onnxruntime.so') ? '/home/dev/onnx/libonnxruntime.so' : path)
    }),
    ['依赖的符号链接指向包外：/pkg/usr/lib/DreamPaper/ort/libonnxruntime.so → /home/dev/onnx/libonnxruntime.so（' + file + '）']
  );

  for (const entry of [
    '$ORIGIN/../../../../opt',
    '$ORIGINBAD',
    'ORIGIN/../lib',
    '$ORIGINX',
    '${ORIGIN',
    '/home/runner/work/dreampaper/src-tauri/target/release'
  ]) {
    const problems = linuxBinaryProblems({ file, dynamic: ` (RUNPATH) Library runpath: [${entry}]`, root: '/pkg' });
    assert.ok(problems.length > 0, entry);
  }
  // 规范化后再判边界：/lib/../home 不能因为前缀是 /lib 而放行。
  const normalized = linuxBinaryProblems({ file, linked: '\tlibevil.so => /lib/../home/runner/libevil.so (0x1)', root: '/pkg' });
  assert.match(normalized.join('\n'), /不在系统库目录也不在包内/);
  // 无 => 的绝对路径行同样要判归属。
  const bare = linuxBinaryProblems({ file, linked: '\t/home/runner/ld-linux-x86-64.so.2 (0x1)', root: '/pkg' });
  assert.match(bare.join('\n'), /不在系统库目录也不在包内/);
  // 相对解析结果一律报错，不默默跳过。
  const relative = linuxBinaryProblems({ file, linked: '\tlibfoo.so => ../libfoo.so (0x1)', root: '/pkg' });
  assert.match(relative.join('\n'), /非绝对路径/);
  const missing = linuxBinaryProblems({ file, linked: '\tlibbar.so => not found', root: '/pkg' });
  assert.match(missing.join('\n'), /缺少动态依赖/);
});

/** 临时目录里的假 gh：只把收到的调用记到 calls.txt，不碰任何远端。 */
function fakeGh(directory) {
  const bin = join(directory, 'bin');
  mkdirSync(bin, { recursive: true });
  writeFileSync(join(bin, 'gh.js'), [
    "const { appendFileSync, writeFileSync } = require('node:fs');",
    "const { join } = require('node:path');",
    "const marker = join(__dirname, '..', 'calls.txt');",
    "appendFileSync(marker, process.argv.slice(2).join(' ') + '\\n');",
    "const url = process.argv[3] ?? '';",
    "if (url.includes('/releases')) process.stdout.write('[]');",
    "else if (url.includes('/git/ref/tags/')) process.stdout.write(JSON.stringify({ object: { type: 'commit', sha: process.env.GITHUB_SHA } }));",
    "else writeFileSync(join(__dirname, '..', 'notes.txt'), '');"
  ].join('\n'));
  writeFileSync(join(bin, 'gh'), '#!/bin/sh\nexec node "$(dirname "$0")/gh.js" "$@"\n', { mode: 0o755 });
  writeFileSync(join(bin, 'gh.cmd'), '@node "%~dp0gh.js" %*\r\n');
  return { bin, marker: join(directory, 'calls.txt') };
}

test('缺少第五个产物时草稿步骤必须阻断，且一次远端调用都不发生', () => {
  const directory = mkdtempSync(join(tmpdir(), 'draft 空格 '));
  try {
    const gh = fakeGh(directory);
    mkdirSync(join(directory, 'release-artifacts'));
    writeFileSync(join(directory, 'package.json'), JSON.stringify({ name: 'dreampaper', version: '0.2.1' }));
    writeFileSync(join(directory, 'CHANGELOG.md'), '# 变更记录\n\n## v0.2.1\n\n测试条目\n');
    const assets = ['dreampaper-0.2.1-setup.exe', 'dreampaper-0.2.1-portable.zip', 'dreampaper-0.2.1-x64-mac.dmg', 'dreampaper-0.2.1-arm64-mac.dmg'];
    for (const name of assets) writeFileSync(join(directory, 'release-artifacts', name), 'asset');
    const run = () => spawnSync(process.execPath, [join(root, 'scripts/draft.mjs')], {
      cwd: directory,
      encoding: 'utf8',
      env: {
        ...process.env,
        PATH: `${gh.bin}${delimiter}${process.env.PATH}`,
        GITHUB_REF_NAME: 'v0.2.1', GITHUB_REPOSITORY: 'owner/repo', GITHUB_SHA: '0'.repeat(40)
      }
    });

    const blocked = run();
    assert.notEqual(blocked.status, 0);
    assert.match(`${blocked.stdout}${blocked.stderr}`, /最终产物集合不完整/);
    assert.ok(!existsSync(gh.marker), '缺产物时不得调用 gh');

    writeFileSync(join(directory, 'release-artifacts', 'dreampaper-0.2.1-amd64.deb'), 'asset');
    const passed = run();
    assert.equal(passed.status, 0, passed.stderr);
    const calls = readFileSync(gh.marker, 'utf8');
    // 先查重再建草稿，且五个资产都随命令上传。
    assert.match(calls, /^api repos\/owner\/repo\/releases\?per_page=100$/m);
    const create = calls.split('\n').find((line) => line.startsWith('release create v0.2.1'));
    assert.ok(create, calls);
    for (const name of [...assets, 'dreampaper-0.2.1-amd64.deb']) assert.ok(create.includes(`release-artifacts/${name}`), `${name}: ${create}`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});

test('升版本只改主包锁文件条目，OCR 版本不动', () => {
  const directory = mkdtempSync(join(tmpdir(), 'version 空格 '));
  try {
    mkdirSync(join(directory, 'src-tauri'), { recursive: true });
    writeFileSync(join(directory, 'package.json'), '{\n  "name": "dreampaper",\n  "version": "0.1.0"\n}\n');
    writeFileSync(join(directory, 'package-lock.json'), JSON.stringify({ name: 'dreampaper', version: '0.1.0', packages: { '': { version: '0.1.0' } } }) + '\n');
    writeFileSync(join(directory, 'src-tauri/tauri.conf.json'), '{\n  "productName": "DreamPaper",\n  "version": "0.1.0"\n}\n');
    writeFileSync(join(directory, 'src-tauri/Cargo.toml'), '[package]\nname = "dreampaper"\nversion = "0.1.0"\nedition = "2021"\n');
    writeFileSync(
      join(directory, 'src-tauri/Cargo.lock'),
      '[[package]]\nname = "dreampaper"\nversion = "0.1.0"\ndependencies = [\n "dreampaper-ocr",\n]\n\n[[package]]\nname = "dreampaper-ocr"\nversion = "0.1.0"\n'
    );
    const result = spawnSync(process.execPath, [join(root, '.github/scripts/set-version.mjs'), '0.9.9'], { cwd: directory, encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.lock'), 'utf8'), /name = "dreampaper"\nversion = "0\.9\.9"/);
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.lock'), 'utf8'), /name = "dreampaper-ocr"\nversion = "0\.1\.0"/);
    writeFileSync(
      join(directory, 'src-tauri/Cargo.lock'),
      '[[package]]\r\nname = "dreampaper"\r\nversion = "0.1.0"\r\ndependencies = [\r\n "dreampaper-ocr",\r\n]\r\n\r\n[[package]]\r\nname = "dreampaper-ocr"\r\nversion = "0.1.0"\r\n'
    );
    const crlf = spawnSync(process.execPath, [join(root, '.github/scripts/set-version.mjs'), '0.9.9'], { cwd: directory, encoding: 'utf8' });
    assert.equal(crlf.status, 0, crlf.stderr);
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.lock'), 'utf8'), /name = "dreampaper"\r\nversion = "0\.9\.9"/);
    assert.match(readFileSync(join(directory, 'src-tauri/Cargo.lock'), 'utf8'), /name = "dreampaper-ocr"\r\nversion = "0\.1\.0"/);
    for (const path of ['package.json', 'src-tauri/tauri.conf.json', 'src-tauri/Cargo.toml']) {
      assert.match(readFileSync(join(directory, path), 'utf8'), /"?0\.9\.9"?/);
    }
    const lock = JSON.parse(readFileSync(join(directory, 'package-lock.json'), 'utf8'));
    assert.equal(lock.version, '0.9.9');
    assert.equal(lock.packages[''].version, '0.9.9');
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
});
