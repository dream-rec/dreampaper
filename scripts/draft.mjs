import { readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { execFileSync } from 'node:child_process';

// 测试把 DREAMPAPER_GH 指到一个 Node 脚本。Windows 上 PATH 前面的 gh.cmd
// 盖不住已安装的 gh.exe，所以不能靠改 PATH 来替换。
function gh(args, inherit = false) {
  const fake = process.env.DREAMPAPER_GH;
  const command = fake ? process.execPath : 'gh';
  const commandArgs = fake ? [fake, ...args] : args;
  const options = inherit ? { stdio: 'inherit' } : { encoding: 'utf8' };
  return execFileSync(command, commandArgs, options);
}

const tag = process.env.GITHUB_REF_NAME;
const repository = process.env.GITHUB_REPOSITORY;
if (!/^v\d+\.\d+\.\d+(?:-[\w.-]+)?$/.test(tag || '') || !repository) throw new Error('发布上下文无效');
const version = tag.slice(1);
if (JSON.parse(readFileSync('package.json', 'utf8')).version !== version) throw new Error('tag 与源码版本不一致');
const expected = ['setup.exe', 'portable.zip', 'x64-mac.dmg', 'arm64-mac.dmg', 'amd64.deb'].map((suffix) => `dreampaper-${version}-${suffix}`).sort();
const files = readdirSync('release-artifacts').sort();
if (JSON.stringify(files) !== JSON.stringify(expected)) throw new Error(`最终产物集合不完整：${files}`);
const releases = JSON.parse(gh(['api', `repos/${repository}/releases?per_page=100`]));
const existing = releases.find((release) => release.tag_name === tag);
if (existing && !existing.draft) throw new Error('该版本已公开，禁止覆盖');
const remote = JSON.parse(gh(['api', `repos/${repository}/git/ref/tags/${tag}`]));
let sha = remote.object.sha;
if (remote.object.type === 'tag') sha = JSON.parse(gh(['api', `repos/${repository}/git/tags/${sha}`])).object.sha;
if (sha !== process.env.GITHUB_SHA) throw new Error('远程 tag 已变化，停止发布');
const changelog = readFileSync('CHANGELOG.md', 'utf8');
const section = changelog.split(`## ${tag}`)[1]?.split(/\n## /)[0]?.trim();
if (!section) throw new Error('缺少当前版本 CHANGELOG');
const notes = `## DreamPaper ${tag}\n\n${section}\n\n## 安装文件\n\n| 文件 | 平台 |\n| --- | --- |\n| dreampaper-${version}-setup.exe | Windows x64 安装版 |\n| dreampaper-${version}-portable.zip | Windows x64 便携版，完整解压后运行 |\n| dreampaper-${version}-x64-mac.dmg | macOS Intel |\n| dreampaper-${version}-arm64-mac.dmg | macOS Apple Silicon |\n| dreampaper-${version}-amd64.deb | Ubuntu 22.04 及以上 x86-64（Debian 系，需 libwebkit2gtk-4.1） |\n\n五类产物均通过最终包内的模型下载、中英文推理、重启后离线识别门禁。OCR 引擎内置，模型首次使用时下载；无需 Python。便携版要求系统已安装 WebView2 Runtime。\n\nLinux 只发布 x86-64 的 .deb，在 Ubuntu 22.04 上构建，可在 22.04 及更新的发行版上安装（需 libwebkit2gtk-4.1）；不提供 RPM 或 AppImage。\n\nmacOS 使用 ad-hoc 签名，未做 Developer ID 公证；Windows 未做商业代码签名。Gatekeeper / SmartScreen 可能提示，请仅从本仓库下载。构建部署目标为 macOS 13.0，但现代 CI runner 不代表最低系统实机测试已经完成。\n\n本项目采用 PolyForm Noncommercial License 1.0.0，仅允许非商业使用。\n`;
const assets = files.map((name) => `release-artifacts/${name}`);
if (!existing) {
  writeFileSync('release-notes.md', notes);
  gh(['release', 'create', tag, '--repo', repository, '--draft', '--verify-tag', '--title', `DreamPaper ${tag}`, '--notes-file', 'release-notes.md', ...assets], true);
} else {
  // 草稿已在发布页改过标题和说明。重跑只替换安装包，不调用 release edit。
  gh(['release', 'upload', tag, '--repo', repository, '--clobber', ...assets], true);
}
