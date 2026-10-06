import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { availableParallelism } from 'node:os';
import { digest, filesIn, locked, root, targetInfo, targets, verifyRuntime } from './runtime.mjs';

const PATCHES = [
  {
    file: 'onnxruntime/core/mlas/lib/qlutgemm.cpp',
    original: 'auto lut_buffer = std::make_unique_for_overwrite<int8_t[]>(lut_buffer_size);',
    compatible: 'auto lut_buffer = std::make_unique<int8_t[]>(lut_buffer_size);'
  },
  {
    file: 'onnxruntime/core/session/model_editor_c_api.cc',
    original: 'auto [ptr_it, ptr_inserted] = graph->initializer_ptrs.insert(tensor);',
    compatible: 'auto ptr_result = graph->initializer_ptrs.insert(tensor);\n  auto ptr_it = ptr_result.first;\n  bool ptr_inserted = ptr_result.second;'
  },
  {
    file: 'onnxruntime/core/session/model_editor_c_api.cc',
    original: 'auto [ptr_it, ptr_inserted] = graph->node_ptrs.insert(node);',
    compatible: 'auto ptr_result = graph->node_ptrs.insert(node);\n  auto ptr_it = ptr_result.first;\n  bool ptr_inserted = ptr_result.second;'
  }
];
const args = process.argv.slice(2);
const explicit = args.indexOf('--target');
const triple = explicit >= 0 ? args[explicit + 1] : Object.keys(targets).find((key) => {
  const info = targetInfo(key);
  return info.platform === process.platform && info.arch === process.arch;
});
const info = targetInfo(triple);
if (info.platform !== process.platform || info.arch !== process.arch) throw new Error('ORT 必须在目标平台原生构建');
const arch = info.arch === 'x64' ? 'x86_64' : 'arm64';
const windows = info.platform === 'win32';
const darwin = info.platform === 'darwin';
const source = resolve(root, `src-tauri/target/gate/onnxruntime-${locked.version}`);
const build = resolve(root, `src-tauri/target/gate/ort-build-${arch}`);
const destination = resolve(root, 'src-tauri/runtime', triple);
const tools = resolve(root, 'src-tauri/target/gate/tools/bin');
const python = process.env.ORT_PYTHON || (existsSync(join(tools, 'python')) ? join(tools, 'python') : 'python');
const cmake = existsSync(join(tools, 'cmake')) ? join(tools, 'cmake') : 'cmake';
const ctest = existsSync(join(tools, 'ctest')) ? join(tools, 'ctest') : 'ctest';
const env = { ...process.env, ...(darwin ? { MACOSX_DEPLOYMENT_TARGET: '13.0' } : {}) };
if (!existsSync(join(source, '.git'))) {
  mkdirSync(source, { recursive: true });
  run('git', ['init', source]);
  run('git', ['-C', source, 'remote', 'add', 'origin', 'https://github.com/microsoft/onnxruntime.git']);
  run('git', ['-C', source, 'fetch', '--depth', '1', 'origin', locked.commit]);
  run('git', ['-C', source, 'checkout', '--detach', 'FETCH_HEAD']);
}
if (capture('git', ['-C', source, 'rev-parse', 'HEAD']).trim() !== locked.commit) throw new Error('ORT 源码提交不匹配');
for (const patch of PATCHES) {
  const path = join(source, patch.file);
  const text = readFileSync(path, 'utf8');
  if (text.split(patch.original).length === 2 && !text.includes(patch.compatible)) {
    writeFileSync(path, text.replace(patch.original, patch.compatible));
  } else if (text.includes(patch.original) || text.split(patch.compatible).length !== 2) {
    throw new Error(`ORT 兼容补丁上下文不匹配：${patch.file}`);
  }
}
const buildArgs = [
  join(source, 'tools/ci_build/build.py'), '--build_dir', build,
  '--config', 'Release', '--update', '--build', '--build_shared_lib', '--targets', 'onnxruntime',
  '--skip_tests', '--skip_pip_install', '--compile_no_warning_as_error', '--no_telemetry',
  '--parallel', String(Math.min(6, availableParallelism())),
  '--cmake_path', cmake, '--ctest_path', ctest,
  '--cmake_generator', windows ? 'Visual Studio 17 2022' : 'Ninja',
  ...(windows ? ['--enable_msvc_static_runtime'] : darwin ? ['--osx_arch', arch, '--apple_deploy_target', '13.0'] : []),
  '--cmake_extra_defines', 'CMAKE_BUILD_TYPE=Release', 'onnxruntime_BUILD_UNIT_TESTS=OFF',
  ...(darwin ? ['CMAKE_OSX_DEPLOYMENT_TARGET=13.0'] : [])
];
const toolchain = {
  python: capture(python, ['--version']).trim(),
  cmake: capture(cmake, ['--version']).trim(),
  // 记录的是实际参与构建的编译器：Linux 走系统 cc（gcc），macOS 才是 clang。
  compiler: windows ? process.env.VCToolsVersion : capture(darwin ? 'clang' : 'cc', ['--version']).trim()
};
run(python, buildArgs);
const outputs = [join(build, 'Release', 'Release'), join(build, 'Release')].filter((dir) => existsSync(join(dir, info.library)));
if (outputs.length !== 1) throw new Error(`ORT 构建产物必须唯一：${outputs}`);
const output = outputs[0];
rmSync(destination, { recursive: true, force: true });
mkdirSync(join(destination, 'licenses'), { recursive: true });
for (const name of readdirSync(output)) {
  if ((windows && name.endsWith('.dll')) || name === info.library) {
    copyFileSync(realpathSync(join(output, name)), join(destination, name));
  }
}
for (const name of ['LICENSE', 'ThirdPartyNotices.txt']) copyFileSync(join(source, name), join(destination, 'licenses', name));
copyFileSync(join(root, 'src-tauri/ocr/THIRD_PARTY.md'), join(destination, 'licenses/OCR.md'));
let inspection;
if (windows) {
  inspection = capture('dumpbin', ['/DEPENDENTS', join(destination, info.library)]);
  if (/VCRUNTIME|MSVCP/i.test(inspection)) throw new Error('ORT 未静态链接 MSVC 运行时');
} else if (darwin) {
  inspection = capture('otool', ['-L', join(destination, info.library)]);
  const buildVersion = capture('vtool', ['-show-build', join(destination, info.library)]);
  const minimum = [...buildVersion.matchAll(/minos\s+(\d+(?:\.\d+)*)/g)].map((match) => match[1]);
  if (!minimum.length || minimum.some((v) => Number(v.split('.')[0]) > 13 || (Number(v.split('.')[0]) === 13 && Number(v.split('.')[1] || 0) > 0))) {
    throw new Error(`ORT 最低系统高于 macOS 13.0：${buildVersion}`);
  }
  inspection += buildVersion;
} else {
  inspection = inspectElf(join(destination, info.library), arch);
}
writeFileSync(join(destination, 'manifest.json'), JSON.stringify({
  version: locked.version, commit: locked.commit, triple,
  recipe_sha256: digest(join(root, 'scripts/ort.mjs')).sha256,
  deployment_target: darwin ? '13.0' : null, toolchain, build_args: buildArgs,
  inspection,
  files: filesIn(destination).map((name) => ({ name, ...digest(join(destination, name)) }))
}, null, 2) + '\n');
verifyRuntime(destination, triple);
console.log(`ORT_BUILD_OK ${destination}`);

/**
 * Linux 产物必须真的是目标架构的 ELF、不带构建机路径，而且依赖在当前镜像里可解析。
 * Tauri 的 deb 打包不会自动推导依赖，所以这里先把“能不能加载”拦在前面。
 */
function inspectElf(path, expectedArch) {
  const header = capture('readelf', ['-h', path]);
  const machine = expectedArch === 'x86_64' ? 'Advanced Micro Devices X86-64' : 'AArch64';
  if (!/Class:\s+ELF64/.test(header)) throw new Error(`ORT 不是 64 位 ELF：${path}`);
  if (!header.includes(machine)) throw new Error(`ORT 架构不符（期望 ${machine}）：\n${header}`);
  const dynamic = capture('readelf', ['-d', path]);
  for (const match of dynamic.matchAll(/\((?:RPATH|RUNPATH)\)[^\n]*\[([^\]]+)\]/g)) {
    throw new Error(`ORT 不得携带 RPATH/RUNPATH（会把构建机路径带进产物）：${match[1]}`);
  }
  const linked = capture('ldd', [path]);
  if (/not found/i.test(linked)) throw new Error(`ORT 动态依赖不可解析：\n${linked}`);
  return `${header}\n${dynamic}\n${linked}`;
}

function capture(command, commandArgs) {
  return execFileSync(command, commandArgs, { encoding: 'utf8', env });
}
function run(command, commandArgs) {
  const result = spawnSync(command, commandArgs, { stdio: 'inherit', env });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} 退出码 ${result.status}`);
}
