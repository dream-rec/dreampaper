import type {
  AppConfig,
  AssetUpload,
  DesignLogEvent,
  JobCreateRequest,
  JobRating,
  JobRecord,
  ReleaseInfo,
  TemplatePackSummary,
  TemplateSummary
} from './types';

const isDesktop =
  typeof window !== 'undefined' &&
  (Boolean((window as { isTauri?: boolean }).isTauri) || '__TAURI_INTERNALS__' in window);

type InvokeFn = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;

let invokeFn: Promise<InvokeFn> | null = null;

/** A backend `AppError`: the message is user-facing, the code is for logic. */
export class IpcError extends Error {
  code: string;
  detail: unknown;
  constructor(message: string, code: string, detail: unknown) {
    super(message);
    this.name = 'IpcError';
    this.code = code;
    this.detail = detail;
  }
}

export function errorCode(error: unknown): string | null {
  return error instanceof IpcError ? error.code : null;
}

async function ipc<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  invokeFn ??= import('@tauri-apps/api/core').then((mod) => mod.invoke as InvokeFn);
  const invoke = await invokeFn;
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    const code =
      error && typeof error === 'object' && 'code' in error ? String((error as { code: unknown }).code) : 'unknown';
    const detail = error && typeof error === 'object' && 'detail' in error ? (error as { detail: unknown }).detail : null;
    throw new IpcError(errorText(error), code, detail);
  }
}

function errorText(error: unknown): string {
  if (typeof error === 'string') return error;
  if (error && typeof error === 'object' && 'message' in error) {
    return String((error as { message: unknown }).message);
  }
  return 'Unexpected error';
}

async function request<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return response.json() as Promise<T>;
}

async function fileBytes(file: File): Promise<number[]> {
  return Array.from(new Uint8Array(await file.arrayBuffer()));
}

export function getConfig() {
  return isDesktop ? ipc<AppConfig>('get_config') : request<AppConfig>('/api/config/models');
}

export function saveConfig(config: AppConfig) {
  if (isDesktop) return ipc<AppConfig>('save_config', { config });
  return request<AppConfig>('/api/config/models', {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(config)
  });
}

export function checkUpdate() {
  return ipc<ReleaseInfo>('check_update');
}

export function listTemplates(kind: string, q: string) {
  if (isDesktop) {
    return ipc<TemplateSummary[]>('list_templates', { kind: kind || 'all', query: q });
  }
  const params = new URLSearchParams();
  if (kind) params.set('kind', kind);
  if (q) params.set('q', q);
  params.set('limit', '60');
  return request<TemplateSummary[]>(`/api/templates?${params.toString()}`);
}

export async function uploadAsset(file: File) {
  if (isDesktop) {
    return ipc<AssetUpload>('import_asset', {
      filename: file.name,
      mimeType: file.type,
      bytes: await fileBytes(file)
    });
  }
  const data = new FormData();
  data.append('file', file);
  return request<AssetUpload>('/api/assets', { method: 'POST', body: data });
}

export function createJob(body: JobCreateRequest) {
  if (isDesktop) return ipc<JobRecord>('create_job', { payload: body });
  return request<JobRecord>('/api/jobs', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body)
  });
}

export function getJob(id: string) {
  if (isDesktop) return ipc<JobRecord>('get_job', { id });
  return request<JobRecord>(`/api/jobs/${id}`);
}

export function desktopAvailable() {
  return isDesktop;
}

/**
 * Subscribe to design-model output as it is produced.
 *
 * Job state is polled every 1.8s, which is enough for a stage name but would
 * turn a streamed answer into a slideshow, so these arrive pushed instead. The
 * returned promise resolves to an unsubscribe function; outside the desktop
 * shell there is no event bus, so it resolves to a no-op and nothing streams.
 */
export async function listenDesignLog(
  handler: (event: DesignLogEvent) => void
): Promise<() => void> {
  if (!isDesktop) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<DesignLogEvent>('job://design', (event) => handler(event.payload));
}

export function listJobs(limit = 20, offset = 0) {
  return ipc<JobRecord[]>('list_jobs', { limit, offset });
}

export function cancelJob(id: string) {
  return ipc<JobRecord>('cancel_job', { id });
}

/** 继续一个失败或已停止的任务：同一条记录重跑，已完成的部分从缓存回放。 */
export function resumeJob(id: string) {
  return ipc<JobRecord>('resume_job', { id });
}

export function deleteJob(id: string) {
  return ipc<null>('delete_job', { id });
}

/** Tag a finished job 优/良/差; `null` clears the tag. The case memory keeps it. */
export function rateJob(id: string, rating: JobRating | null) {
  return ipc<JobRecord>('rate_job', { id, rating });
}

export function deleteTemplates(ids: string[]) {
  return ipc<number>('delete_templates', { ids });
}

export async function saveAsset(assetId: string, defaultFilename: string): Promise<void> {
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({ defaultPath: defaultFilename });
  if (!path) return;
  return ipc('save_asset', { assetId, path });
}

export async function importTemplateImage(
  file: File,
  kind: string,
  category: string,
  visualIntent: string,
  contentSummary: string
) {
  return ipc<TemplateSummary>('import_template_image', {
    filename: file.name,
    mimeType: file.type,
    bytes: await fileBytes(file),
    kind,
    category: category || null,
    visualIntent: visualIntent || null,
    contentSummary: contentSummary || null
  });
}

export function importTemplatePack(path: string) {
  return ipc<TemplatePackSummary>('import_template_pack', { path });
}

export async function pickDirectory(title: string): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({ directory: true, multiple: false, title });
  return typeof selected === 'string' ? selected : null;
}

/**
 * Open a URL in the user's default browser.
 *
 * Inside the Tauri webview a plain `<a target="_blank">` is a silent no-op:
 * there is no window.open handler, so the click produces no navigation and no
 * error. The URL has to be handed to the OS instead. The capability file keeps
 * this limited to the external resources the UI actually links to.
 */
export async function openExternal(url: string): Promise<void> {
  if (!isDesktop) {
    window.open(url, '_blank', 'noreferrer');
    return;
  }
  return ipc('plugin:opener|open_url', { url });
}

/**
 * Open a generated image with the OS default viewer.
 *
 * Same root cause as openExternal: the `<a target="_blank">` preview link does
 * nothing inside the webview. The backend resolves the id against the asset
 * table, so the webview never handles a filesystem path.
 */
export async function openArtifact(assetId: string): Promise<void> {
  return ipc('open_artifact', { artifactId: assetId });
}


// ---- Workbench --------------------------------------------------------------

export function listWorkbenchProjects() {
  return ipc<import('./workbench/types').ProjectSummary[]>('list_workbench_projects');
}

export function openWorkbenchProject(
  source: import('./workbench/types').ProjectSource,
  forceNew = false
) {
  return ipc<import('./workbench/types').OpenResult>('open_workbench_project', { source, forceNew });
}

export function copyWorkbenchProject(projectId: string) {
  return ipc<import('./workbench/types').ProjectDetail>('copy_workbench_project', { projectId });
}

export function getWorkbenchProject(projectId: string) {
  return ipc<import('./workbench/types').ProjectDetail>('get_workbench_project', { projectId });
}

export function saveWorkbenchProject(
  projectId: string,
  baseRevision: number,
  document: import('./workbench/types').ProjectDoc
) {
  return ipc<import('./workbench/types').ProjectDetail>('save_workbench_project', {
    projectId,
    baseRevision,
    document
  });
}

export function renameWorkbenchProject(projectId: string, name: string) {
  return ipc<import('./workbench/types').ProjectSummary>('rename_workbench_project', { projectId, name });
}

export function deleteWorkbenchProject(projectId: string) {
  return ipc<import('./workbench/types').DeleteResult>('delete_workbench_project', { projectId });
}

export function analyzeWorkbenchRegion(
  projectId: string,
  rect: import('./workbench/types').PixelRect,
  shape: import('./workbench/types').Shape
) {
  return ipc<import('./workbench/types').RegionAnalysis>('analyze_workbench_region', { projectId, rect, shape });
}

/** D1: OCR on a project region; the host crops, pads and runs the sidecar. */
export function recognizeWorkbenchRegion(
  projectId: string,
  rect: import('./workbench/types').PixelRect,
  background: string | null,
  requestId: number
) {
  return ipc<import('./workbench/types').RecognizeResult>('recognize_workbench_region', {
    projectId,
    rect,
    background,
    requestId
  });
}

export function cancelWorkbenchOcr(requestId: number) {
  return ipc<boolean>('cancel_workbench_ocr', { requestId });
}

export function measureWorkbenchText(spec: import('./workbench/types').TextSpec) {
  return ipc<import('./workbench/types').TextLayout>('measure_workbench_text', { spec });
}

export function listWorkbenchFonts(sample?: string) {
  return ipc<import('./workbench/types').FontInfo[]>('list_workbench_fonts', { sample: sample ?? null });
}

export function previewWorkbenchExport(projectId: string, document: import('./workbench/types').ProjectDoc) {
  return ipc<import('./workbench/types').ExportPreview>('preview_workbench_export', { projectId, document });
}

export function exportWorkbenchProject(
  projectId: string,
  document: import('./workbench/types').ProjectDoc,
  path: string
) {
  return ipc<import('./workbench/types').ExportRecord>('export_workbench_project', { projectId, document, path });
}

export function workbenchStorage() {
  return ipc<import('./workbench/types').StorageStats>('workbench_storage');
}

export function cleanupWorkbenchAssets() {
  return ipc<import('./workbench/types').CleanupResult>('cleanup_workbench_assets');
}

export function getOcrPackageStatus() {
  return ipc<import('./workbench/types').OcrPackageStatus>('get_ocr_package_status');
}

export function installOcrPackage() {
  return ipc<null>('install_ocr_package');
}

export function cancelOcrPackageInstall() {
  return ipc<null>('cancel_ocr_package_install');
}

export function removeOcrPackage() {
  return ipc<null>('remove_ocr_package');
}

/** Progress of the OCR model download; resolves to an unsubscribe function. */
export async function listenOcrProgress(
  handler: (progress: import('./workbench/types').OcrProgress) => void
): Promise<() => void> {
  if (!isDesktop) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<import('./workbench/types').OcrProgress>('ocr://progress', (event) => handler(event.payload));
}

export interface ExportProgress {
  project_id: string;
  stage: 'compose' | 'encode' | 'write' | 'done';
  percent: number;
}

export async function listenExportProgress(handler: (progress: ExportProgress) => void): Promise<() => void> {
  if (!isDesktop) return () => {};
  const { listen } = await import('@tauri-apps/api/event');
  return listen<ExportProgress>('workbench://export-progress', (event) => handler(event.payload));
}

/** Ask for an image file to open in the workbench; null when dismissed. */
export async function pickImageFile(title: string): Promise<string | null> {
  const { open } = await import('@tauri-apps/plugin-dialog');
  const selected = await open({
    multiple: false,
    directory: false,
    title,
    filters: [{ name: 'Image', extensions: ['png', 'jpg', 'jpeg', 'webp'] }]
  });
  return typeof selected === 'string' ? selected : null;
}

/** Ask where to write an export; null when dismissed. */
export async function pickSavePath(defaultFilename: string): Promise<string | null> {
  const { save } = await import('@tauri-apps/plugin-dialog');
  const path = await save({ defaultPath: defaultFilename, filters: [{ name: 'PNG', extensions: ['png'] }] });
  return path ?? null;
}
