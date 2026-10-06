import { memo, useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';
import {
  cancelJob,
  createJob,
  desktopAvailable,
  getConfig,
  getJob,
  listenDesignLog,
  listTemplates,
  openArtifact,
  rateJob,
  resumeJob,
  saveAsset,
  saveConfig,
  uploadAsset
} from './api';
import type { AppConfig, AssetUpload, JobDesignLog, JobRating, JobRecord, ModelProfile, TemplateSummary } from './types';
import { saveSimpleMode, simpleModePreference } from './desktop/prefs';

export type Lang = 'zh' | 'en';
type UiMode = 'paper' | 'ppt' | 'settings';
export type PaperFigureState = {
  kind: string;
  query: string;
  selected: string[];
  title: string;
  description: string;
  aspectRatio: string;
  layoutFidelity: 'strict' | 'balanced' | 'loose';
  styleStrength: 'high' | 'medium' | 'low';
  custom: string;
};
export type PptSlideState = {
  asset: AssetUpload | null;
  materials: AssetUpload[];
  material: string;
  pages: number;
  custom: string;
};
type StateUpdater<T> = (next: T | ((current: T) => T)) => void;

export const defaultPaperState: PaperFigureState = {
  kind: 'diagram',
  query: '',
  selected: [],
  title: '',
  description: '',
  aspectRatio: 'inherit',
  layoutFidelity: 'balanced',
  styleStrength: 'high',
  custom: ''
};

export const defaultPptState: PptSlideState = {
  asset: null,
  materials: [],
  material: '',
  pages: 1,
  custom: ''
};

export const emptyConfig: AppConfig = {
  version: 1,
  active_design_profile: 'design-default',
  active_implement_profile: 'implement-default',
  active_search_profile: 'search-default',
  proxy_url: '',
  ppt_page_plan_concurrency: null,
  ppt_image_concurrency: null,
  model_profiles: []
};

export const copy = {
  zh: {
    nav: { paper: '科研图', ppt: '幻灯片', settings: '设置' },
    simple: { label: '简单模式', hint: '不调用制图模型，只输出最终制图提示词' },
    brandSub: 'figure / slide',
    common: {
      protocol: '协议',
      baseUrl: 'URL',
      model: '模型',
      apiKey: '密钥',
      save: '保存',
      saved: '已保存',
      saving: '保存中...',
      redacted: '保存后脱敏',
      submitFailed: '提交失败',
      uploadFailed: '上传失败',
      choose: '选择',
      noFile: '未选择',
      uploadedFiles: '已上传'
    },
    settings: { design: 'Design', implement: 'Implement', search: 'Search', searchHint: '幻灯片视觉素材检索。duckduckgo 免密钥，端点写死、不用填 URL；tavily 填 API key；grok_search 就是 OpenAI 兼容的 chat completions（grok2api 即此协议），模型填部署里自带 x_search / web_search 联网能力的模型 ID（例如 grok-build-0.1）。三种协议各自保存自己的配置，切换协议不会互相覆盖，也不会带上上一个协议的值。duckduckgo 走的是它的无 JS 页面，被人机验证拦住时会直接报错，长期用建议换 tavily 或 grok_search。', proxyAndConcurrency: '代理与并发', proxy: '代理', proxyHint: '本地代理地址，例如 http://127.0.0.1:7890；留空表示不指定代理。', concurrency: '幻灯片并发', concurrencyHint: '留空表示跟随本次输入的幻灯片页数；实际并发不会超过页数。制图建议先设为 1，降低网关 502。', pagePlanConcurrency: '规划并发', imageConcurrency: '制图并发', defaultByPages: '默认=页数', size: '尺寸', quality: '质量', format: '格式', ratio: '比例', clarity: '清晰度', tendency: '倾向', version: '版本', timeout: '超时(秒)', timeoutHint: '同步出图可能较久，implement 建议 600–900；grok_search 要让上游模型自己联网检索，建议 ≥120 秒。', retries: '重试次数', maxResults: '结果数', stream: '流式', streamOn: '开启', keySet: '密钥已配置', keyNone: '未配置密钥', proxySet: '代理已配置', proxyNone: '未设置代理', notSet: '未设置' },
    paper: { title: '科研图', intro: '选择 template 作为 few-shot 风格参考。', figureTitle: '标题', description: '方法', ratio: '比例', fidelity: '布局', strength: '风格', custom: '约束', customHint: '可选，用于补充禁用元素、强调风格、文字限制或审稿要求。', generate: '生成', generating: '生成中…', search: '搜索', kind: '类型', inherited: '继承', submitted: '科研图任务已提交' },
    ppt: { title: '幻灯片', intro: '上传 template，分析母版，再批量生成页面。', template: '母版', pages: '页数', material: '资料', materialFile: '附件', materialHint: '可输入文字，也可上传 pdf、docx、txt、md、csv 等资料。', custom: '约束', customHint: '可选，用于补充页数结构、禁用元素、术语、颜色或展示重点。', generate: '生成', generating: '生成中…', submitted: '幻灯片任务已提交', uploading: '上传中...', uploaded: '已上传' },
    result: { title: 'Result', waiting: '等待中', progress: '进度', current: '当前', step: '当前步骤', failed: '失败', completed: '完成', queued: '排队中', running: '运行中', cancelled: '已停止', preview: '预览', download: '下载', of: '/', elapsed: '已用时', stop: '停止任务', stopping: '停止中…', retry: '重试', resume: '继续任务', resuming: '正在继续…', resumeFailed: '继续失败', stopped: '任务已停止', stopFailed: '停止失败', saveFailed: '保存失败', previewFailed: '预览失败', editWorkbench: '在工作台编辑', errorTitle: '故障诊断', errorRole: '相关配置', errorProfile: '配置名称', errorModel: '模型', errorEndpoint: '请求地址', errorStatus: 'HTTP 状态', errorStage: '失败阶段', errorSuggestion: '处理建议', flowLog: '生成过程', searchStage: 'Search', designStage: 'Design', implementStage: 'Implement', searchHint: '检索原始元素，查看描述与参考来源。', designHint: '母版分析、大纲与内容规划。', implementEmpty: '暂无最终产物。', flowQuery: (n: number) => `查询 ${n}`, queryContent: '查询内容', queryResult: '查询结果', stepEmpty: '这一步没有返回内容', stepStreaming: '接收中…', pagePrep: (n: number) => `准备第 ${n} 页规划`, pagePlan: (n: number) => `规划第 ${n} 页`, pageRender: (n: number) => `生成第 ${n} 页`, stages: {
      started: '准备中', queued: '排队中', resuming: '继续上次进度', completed: '任务完成', failed: '任务失败', cancelled: '任务已停止',
      paper_validate: '校验输入', paper_templates: '读取参考模板', paper_memory: '查找相似案例',
      paper_visual_assets: '查询视觉素材', paper_structure_prompt: '准备母版结构分析', paper_structure: '分析母版结构',
      paper_structure_parse: '校验结构规划', paper_advisor: '参考历史案例', paper_prompt: '准备内容填充',
      paper_design: '生成制图方案', paper_parse: '校验制图方案', paper_implement: '生成图片', paper_save: '保存图片',
      ppt_validate: '校验输入', ppt_template: '读取母版', ppt_material: '整理资料', ppt_memory: '查找相似案例',
      ppt_visual_assets: '查询视觉素材', ppt_analyze: '分析母版', ppt_parse_template: '校验母版分析',
      ppt_advisor: '参考历史案例', ppt_outline_prompt: '准备大纲规划', ppt_outline: '规划整套大纲',
      ppt_parse_outline: '校验大纲', ppt_page_plan_queue: '排队规划页面', ppt_merge_pages: '汇总页面规划',
      ppt_implement_queue: '排队生成图片', ppt_save: '保存图片', memory_record: '记录案例'
    }, rate: '评价本次结果', rateHint: '评价会记入案例库，后续相似任务由 advisor 参考', rateGood: '优', rateFair: '良', ratePoor: '差', rateFailed: '评分失败', finalPrompt: '最终制图提示词', finalPromptHint: '简单模式停在这一步：这是可直接复制使用的最终制图提示词，本次没有调用制图模型。', finalPromptPage: (n: number) => `第 ${n} 页`, copy: '复制', copied: '已复制', copyFailed: '复制失败' }
  },
  en: {
    nav: { paper: 'Figure', ppt: 'Slide', settings: 'Settings' },
    simple: { label: 'Simple mode', hint: 'Skip the drawing model and output the final drawing prompt only' },
    brandSub: 'figure / slide',
    common: {
      protocol: 'Protocol',
      baseUrl: 'URL',
      model: 'Model',
      apiKey: 'Key',
      save: 'Save',
      saved: 'Saved',
      saving: 'Saving...',
      redacted: 'Redacted after save',
      submitFailed: 'Submit failed',
      uploadFailed: 'Upload failed',
      choose: 'Choose',
      noFile: 'No file',
      uploadedFiles: 'Uploaded'
    },
    settings: { design: 'Design', implement: 'Implement', search: 'Search', searchHint: "Slide visual grounding search. duckduckgo needs no key and no URL (its endpoint is fixed); tavily needs an API key; grok_search is an OpenAI-compatible chat completions endpoint (grok2api is exactly that): fill in a model ID that carries x_search / web_search itself (e.g. grok-build-0.1). Each protocol keeps its own settings: switching protocols neither overwrites nor carries over the others. duckduckgo reads its no-JS page, so an anti-bot challenge fails the job — for regular use prefer tavily or grok_search.", proxyAndConcurrency: 'Proxy & concurrency', proxy: 'Proxy', proxyHint: 'Local proxy URL, e.g. http://127.0.0.1:7890. Leave empty to disable explicit proxy.', concurrency: 'Slide concurrency', concurrencyHint: 'Leave empty to follow the current Slide page count; actual concurrency will not exceed pages. Prefer image concurrency = 1 to reduce 502s.', pagePlanConcurrency: 'Plan workers', imageConcurrency: 'Image workers', defaultByPages: 'default=pages', size: 'Size', quality: 'Quality', format: 'Format', ratio: 'Ratio', clarity: 'Sharpness', tendency: 'Quality', version: 'Version', timeout: 'Timeout (s)', timeoutHint: 'Sync image APIs can be slow; implement often needs 600–900s. grok_search makes the upstream model search the web, so give it ≥120s.', retries: 'Retries', maxResults: 'Results', stream: 'Stream', streamOn: 'On', keySet: 'Key set', keyNone: 'No key', proxySet: 'Proxy set', proxyNone: 'No proxy', notSet: 'Not set' },
    paper: { title: 'Figure', intro: 'Choose templates as few-shot visual references.', figureTitle: 'Title', description: 'Method', ratio: 'Ratio', fidelity: 'Layout', strength: 'Style', custom: 'Rules', customHint: 'Optional constraints for banned elements, style emphasis, text limits, or review requirements.', generate: 'Generate', generating: 'Generating…', search: 'Search', kind: 'Type', inherited: 'Template', submitted: 'Figure job submitted' },
    ppt: { title: 'Slide', intro: 'Upload a template, analyze the master, then generate pages.', template: 'Master', pages: 'Pages', material: 'Material', materialFile: 'Files', materialHint: 'Enter text or upload pdf, docx, txt, md, csv, and other common materials.', custom: 'Rules', customHint: 'Optional constraints for page structure, banned elements, terms, colors, or focus.', generate: 'Generate', generating: 'Generating…', submitted: 'Slide job submitted', uploading: 'Uploading...', uploaded: 'Uploaded' },
    result: { title: 'Result', waiting: 'Waiting', progress: 'Progress', current: 'Current', step: 'Current step', failed: 'Failed', completed: 'Completed', queued: 'Queued', running: 'Running', cancelled: 'Stopped', preview: 'Preview', download: 'Download', of: '/', elapsed: 'Elapsed', stop: 'Stop job', stopping: 'Stopping…', retry: 'Retry', resume: 'Continue', resuming: 'Resuming…', resumeFailed: 'Could not continue', stopped: 'Job stopped', stopFailed: 'Stop failed', saveFailed: 'Save failed', previewFailed: 'Preview failed', editWorkbench: 'Edit in workbench', errorTitle: 'Failure diagnosis', errorRole: 'Related configuration', errorProfile: 'Profile', errorModel: 'Model', errorEndpoint: 'Endpoint', errorStatus: 'HTTP status', errorStage: 'Failed stage', errorSuggestion: 'Suggested action', flowLog: 'Generation steps', searchStage: 'Search', designStage: 'Design', implementStage: 'Implement', searchHint: 'Original elements, descriptions and reference sources.', designHint: 'Master analysis, outline and content planning.', implementEmpty: 'No final output yet.', flowQuery: (n: number) => `Query ${n}`, queryContent: 'Query', queryResult: 'Query results', stepEmpty: 'This step returned nothing', stepStreaming: 'Receiving…', pagePrep: (n: number) => `Prepare page ${n}`, pagePlan: (n: number) => `Plan page ${n}`, pageRender: (n: number) => `Render page ${n}`, stages: {
      started: 'Getting ready', queued: 'Queued', resuming: 'Resuming from the last step', completed: 'Done', failed: 'Failed', cancelled: 'Stopped',
      paper_validate: 'Checking input', paper_templates: 'Reading reference templates', paper_memory: 'Looking up similar jobs',
      paper_visual_assets: 'Researching visual references', paper_structure_prompt: 'Preparing master analysis', paper_structure: 'Analysing the master',
      paper_structure_parse: 'Checking the structure plan', paper_advisor: 'Consulting past jobs', paper_prompt: 'Preparing content',
      paper_design: 'Drafting the figure plan', paper_parse: 'Checking the figure plan', paper_implement: 'Drawing the figure', paper_save: 'Saving the figure',
      ppt_validate: 'Checking input', ppt_template: 'Reading the master', ppt_material: 'Preparing the material', ppt_memory: 'Looking up similar jobs',
      ppt_visual_assets: 'Researching visual references', ppt_analyze: 'Analysing the master', ppt_parse_template: 'Checking the master analysis',
      ppt_advisor: 'Consulting past jobs', ppt_outline_prompt: 'Preparing the outline', ppt_outline: 'Planning the outline',
      ppt_parse_outline: 'Checking the outline', ppt_page_plan_queue: 'Planning pages', ppt_merge_pages: 'Merging page plans',
      ppt_implement_queue: 'Rendering pages', ppt_save: 'Saving pages', memory_record: 'Saving the case'
    }, rate: 'Rate this result', rateHint: 'Stored with the case; the advisor weighs it on similar future tasks', rateGood: 'Good', rateFair: 'Fair', ratePoor: 'Poor', rateFailed: 'Rating failed', finalPrompt: 'Final drawing prompt', finalPromptHint: 'Simple mode stopped here: this is the final drawing prompt, ready to copy. No drawing model was called.', finalPromptPage: (n: number) => `Page ${n}`, copy: 'Copy', copied: 'Copied', copyFailed: 'Copy failed' }
  }
} as const;

const PAPER_STAGE_WEIGHTS: Record<string, number> = {
  queued: 4,
  started: 8,
  // 继续上次进度从一个很低的基数重新向上走，重放的步骤会很快把它推回原处。
  resuming: 1,
  paper_validate: 11,
  paper_templates: 14,
  paper_memory: 17,
  paper_visual_assets: 21,
  paper_structure_prompt: 24,
  paper_structure: 34,
  paper_structure_parse: 39,
  paper_advisor: 42,
  paper_prompt: 45,
  paper_design: 58,
  paper_parse: 68,
  paper_implement: 86,
  paper_save: 94,
  memory_record: 97,
  completed: 100,
  failed: 100
};

const PPT_STAGE_WEIGHTS: Record<string, number> = {
  queued: 3,
  started: 6,
  resuming: 1,
  ppt_validate: 10,
  ppt_template: 14,
  ppt_material: 18,
  ppt_memory: 20,
  ppt_visual_assets: 22,
  ppt_analyze: 30,
  ppt_parse_template: 36,
  ppt_advisor: 38,
  ppt_outline_prompt: 40,
  ppt_outline: 48,
  ppt_parse_outline: 52,
  ppt_page_plan_queue: 56,
  ppt_merge_pages: 72,
  ppt_implement_queue: 76,
  ppt_save: 95,
  memory_record: 97,
  completed: 100,
  failed: 100
};

export function isJobSettled(status: JobRecord['status']) {
  return status === 'succeeded' || status === 'failed' || status === 'cancelled';
}

export function useJobPolling(job: JobRecord | null, setJob: (job: JobRecord) => void, onError: (message: string) => void) {
  useEffect(() => {
    if (!job || isJobSettled(job.status)) return;
    const jobId = job.id;
    let cancelled = false;
    const timer = window.setInterval(() => {
      getJob(jobId)
        .then((next) => {
          // 切换任务后迟到的响应不能覆盖当前任务。
          if (!cancelled && next.id === jobId) setJob(next);
        })
        .catch((error) => {
          if (!cancelled) onError(error instanceof Error ? error.message : String(error));
        });
    }, 1800);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [job, setJob, onError]);
}

export type DesignCard = {
  step: string;
  label: string;
  text: string;
  status: 'running' | 'succeeded' | 'failed';
  timestamp?: string;
};

/** Merge persisted steps with live output, so polling also works without desktop events. */
export function useDesignLogs(job: JobRecord | null): DesignCard[] {
  const jobId = job?.id ?? null;
  const [streamed, setStreamed] = useState<FinalPromptStream>({ jobId: null, cards: [] });

  useEffect(() => {
    if (!jobId) return;
    let stop: (() => void) | null = null;
    let cancelled = false;
    listenDesignLog((event) => {
      if (cancelled || event.job_id !== jobId || !isDesignStep(event.step)) return;
      setStreamed((current) => {
        const cards = current.jobId === jobId ? current.cards : [];
        const index = cards.findIndex((card) => card.step === event.step);
        const previous = cards[index];
        if (event.kind === 'begin' && previous) return current;
        const next: DesignCard = {
          step: event.step,
          label: event.label || previous?.label || event.step,
          timestamp: event.timestamp,
          text: event.kind === 'begin' || event.kind === 'reset' ? ''
            : event.kind === 'delta' ? (previous?.text ?? '') + event.text : event.text,
          status: event.kind === 'end' ? (event.status === 'failed' ? 'failed' : 'succeeded') : 'running'
        };
        return { jobId, cards: index < 0 ? [...cards, next] : replaceAt(cards, index, next) };
      });
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [jobId]);

  return useMemo(
    () => mergeTracedSteps(jobId, job?.design_logs, streamed, isDesignStep, stepOrder, true),
    [jobId, job?.design_logs, streamed]
  );
}

/**
 * Simple Mode stores its one product under the first prefix; every search call
 * records its upstream traffic under the other two.
 *
 * The log table is reused so history, streaming and cleanup keep working, but
 * these rows are job output or upstream diagnostics rather than a view into a
 * design call, so they are kept out of the collapsible design log and shown in
 * their own blocks.
 */
const FINAL_PROMPT_PREFIX = 'final_prompt_';
const SEARCH_REQUEST_PREFIX = 'search_request_';
const SEARCH_RESPONSE_PREFIX = 'search_response_';

function isFinalPrompt(step: string): boolean {
  return step.startsWith(FINAL_PROMPT_PREFIX);
}

function isSearchTrace(step: string): boolean {
  return step.startsWith(SEARCH_REQUEST_PREFIX) || step.startsWith(SEARCH_RESPONSE_PREFIX);
}

function isDesignStep(step: string): boolean {
  return !isFinalPrompt(step) && !isSearchTrace(step);
}

function stepIndex(prefix: string, step: string): number {
  const index = Number.parseInt(step.slice(prefix.length), 10);
  return Number.isFinite(index) ? index : 0;
}

function finalPromptPage(step: string): number {
  return stepIndex(FINAL_PROMPT_PREFIX, step);
}

function searchTraceIndex(step: string): number {
  return stepIndex(step.startsWith(SEARCH_REQUEST_PREFIX) ? SEARCH_REQUEST_PREFIX : SEARCH_RESPONSE_PREFIX, step);
}

/** Live rows for one job, tagged with that job so a switch can be spotted. */
export type FinalPromptStream = { jobId: string | null; cards: DesignCard[] };

/**
 * The stored-and-streamed rows of `jobId` that `matches`.
 *
 * The stored rows win, live `end` events fill the gap between a row being
 * written and the poll catching up, and a stream belonging to another job is
 * ignored outright. Both predicates are module-level, so their identity is
 * stable across renders.
 */
function useTracedSteps(
  job: JobRecord | null,
  matches: (step: string) => boolean,
  index: (step: string) => number
): DesignCard[] {
  const jobId = job?.id ?? null;
  const recorded = job?.design_logs;
  const [streamed, setStreamed] = useState<FinalPromptStream>({ jobId: null, cards: [] });

  useEffect(() => {
    if (!jobId) return;
    let stop: (() => void) | null = null;
    let cancelled = false;
    listenDesignLog((event) => {
      // 只认完整结束事件：这类步骤都是一次性写入，没有增量可拼。
      if (cancelled || event.job_id !== jobId || event.kind !== 'end' || !matches(event.step)) return;
      setStreamed((current) => {
        const cards = current.jobId === jobId ? current.cards : [];
        const at = cards.findIndex((card) => card.step === event.step);
        const previous = cards[at];
        const settled: DesignCard = {
          step: event.step,
          label: event.label || previous?.label || event.step,
          text: event.text,
          status: event.status === 'failed' ? 'failed' : 'succeeded'
        };
        return { jobId, cards: at < 0 ? [...cards, settled] : replaceAt(cards, at, settled) };
      });
    })
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [jobId, matches]);

  return useMemo(
    () => mergeTracedSteps(jobId, recorded, streamed, matches, index),
    [jobId, recorded, streamed, matches, index]
  );
}

/**
 * The final drawing prompts Simple Mode produced for this job.
 */
export function useFinalPrompts(job: JobRecord | null): DesignCard[] {
  return useTracedSteps(job, isFinalPrompt, finalPromptPage);
}

/** What every grok_search call actually sent upstream and got back. */
export function useSearchTraces(job: JobRecord | null): DesignCard[] {
  return useTracedSteps(job, isSearchTrace, searchTraceIndex);
}

/**
 * One row of the flow: a search round, a model step, or the final prompt.
 *
 * `request`/`response` are only set for search rounds, `text` for the other two.
 */
export type FlowEntry = {
  key: string;
  kind: 'search' | 'step' | 'prompt';
  phase: 'search' | 'design' | 'implement';
  title: string;
  status: DesignCard['status'];
  step: string;
  text?: string;
  request?: string;
  response?: string;
};

/// Stages in the order the pipeline reaches them; anything unknown keeps a
/// middle rank so an unrecognised step never jumps ahead of a known one.
const FLOW_ORDER: Record<string, number> = {
  paper_visual_assets: 1,
  paper_structure: 2,
  advisor: 3,
  paper_design: 4,
  ppt_visual_assets: 1,
  ppt_analyze: 2,
  ppt_outline: 4
};

function stepOrder(step: string): number {
  const page = step.match(/^ppt_page_plan_(\d+)$/);
  if (page) return 100 + Number(page[1]);
  return FLOW_ORDER[step] ?? 50;
}

/** The whole flow for one job: queries first, then steps, then the prompt. */
export function buildFlowEntries(
  steps: DesignCard[],
  searches: SearchRound[],
  prompts: DesignCard[],
  t: typeof copy[Lang],
  mode: JobRecord['mode']
): FlowEntry[] {
  const entries: FlowEntry[] = searches.map((round) => ({
    key: `search-${round.index}`,
    kind: 'search',
    phase: 'search',
    title: round.label || t.result.flowQuery(round.index),
    status: round.status ?? (round.response === undefined ? 'running' : 'succeeded'),
    step: `search-${round.index}`,
    request: round.request,
    response: round.response
  }));
  for (const card of [...steps].sort((a, b) => stepOrder(a.step) - stepOrder(b.step))) {
    entries.push({
      key: card.step,
      kind: 'step',
      phase: card.step.endsWith('_visual_assets') ? 'search' : 'design',
      title: card.label || card.step,
      status: card.status,
      step: card.step,
      text: card.text
    });
  }
  // Page numbers are not always the order the caller has them in, so sort here
  // rather than trusting the array.
  for (const card of [...prompts].sort((a, b) => finalPromptPage(a.step) - finalPromptPage(b.step))) {
    entries.push({
      key: card.step,
      kind: 'prompt',
      phase: 'implement',
      title: mode === 'ppt_slide' ? t.result.finalPromptPage(finalPromptPage(card.step)) : card.label || t.result.finalPrompt,
      status: card.status,
      step: card.step,
      text: card.text
    });
  }
  return entries;
}

/** A pipeline stage code in the user's words; unknown codes pass through. */
export function stageLabel(stage: string | undefined, t: typeof copy[Lang]): string {
  if (!stage) return '';
  const named = (t.result.stages as Record<string, string | undefined>)[stage];
  if (named) return named;
  const page = stage.match(/^ppt_page_plan_(\d+)$/);
  if (page) return t.result.pagePlan(Number(page[1]));
  const prep = stage.match(/^ppt_page_prompt_(\d+)$/);
  if (prep) return t.result.pagePrep(Number(prep[1]));
  const render = stage.match(/^ppt_implement_(\d+)$/);
  if (render) return t.result.pageRender(Number(render[1]));
  return stage;
}

/** 制图阶段一进入就显示 Implement：图片要跑很久，不能等到保存回来才出现卡片。 */
const IMPLEMENT_STAGES = /^(?:paper_implement|paper_save|ppt_implement_queue|ppt_implement_\d+|ppt_save)$/;

export function implementStarted(job: JobRecord): boolean {
  const stages = [job.stage, job.error?.stage, ...(job.events ?? []).map((event) => event.stage)];
  return stages.some((stage) => !!stage && IMPLEMENT_STAGES.test(stage));
}

/** One search round: the request body paired with the upstream response. */
export type SearchRound = { index: number; label: string; request?: string; response?: string; status?: DesignCard['status'] };

export function searchRounds(cards: DesignCard[]): SearchRound[] {
  const rounds = new Map<number, SearchRound>();
  for (const card of cards) {
    const index = searchTraceIndex(card.step);
    const round = rounds.get(index) ?? { index, label: '' };
    if (card.step.startsWith(SEARCH_REQUEST_PREFIX)) {
      round.request = card.text;
      round.label = card.label;
      if (card.status === 'failed') round.status = 'failed';
    } else {
      round.response = card.text;
      round.label ||= card.label;
      if (round.status !== 'failed') round.status = card.status;
    }
    rounds.set(index, round);
  }
  return [...rounds.values()].sort((a, b) => a.index - b.index);
}

/** The rows to show for `jobId` — see `useTracedSteps`. */
export function mergeTracedSteps(
  jobId: string | null,
  recorded: JobDesignLog[] | undefined,
  streamed: FinalPromptStream,
  matches: (step: string) => boolean,
  index: (step: string) => number,
  preferNewerStream = false
): DesignCard[] {
  const cards = new Map<string, DesignCard>();
  if (streamed.jobId === jobId) {
    for (const card of streamed.cards) cards.set(card.step, card);
  }
  for (const entry of recorded ?? []) {
    if (!matches(entry.step)) continue;
    // A design retry can start after the previous attempt was persisted. Do not
    // let that older snapshot hide the retry, but let the newer final poll win.
    const live = cards.get(entry.step);
    if (preferNewerStream && live?.timestamp && Date.parse(live.timestamp) > Date.parse(entry.timestamp)) continue;
    // Final prompts and transport traces always prefer the complete stored row.
    cards.set(entry.step, {
      step: entry.step,
      label: entry.label,
      text: entry.content,
      status: entry.status === 'failed' ? 'failed' : 'succeeded'
    });
  }
  return [...cards.values()].sort((a, b) => index(a.step) - index(b.step));
}

/** The final prompts of one job (kept for the existing callers and tests). */
export function mergeFinalPrompts(
  jobId: string | null,
  recorded: JobDesignLog[] | undefined,
  streamed: FinalPromptStream
): DesignCard[] {
  return mergeTracedSteps(jobId, recorded, streamed, isFinalPrompt, finalPromptPage);
}

function replaceAt<T>(items: T[], index: number, value: T): T[] {
  const next = items.slice();
  next[index] = value;
  return next;
}
export function useElapsedSeconds(job: JobRecord | null): number {
  const jobId = job?.id ?? null;
  const started = job ? Date.parse(job.created_at) : Number.NaN;
  const settled = job ? isJobSettled(job.status) : true;
  const finished = job && settled ? Date.parse(job.updated_at) : Number.NaN;
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!jobId || settled) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [jobId, settled]);

  if (!job || Number.isNaN(started)) return 0;
  const end = Number.isNaN(finished) ? now : finished;
  return Math.max(0, Math.round((end - started) / 1000));
}

export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const pad = (value: number) => String(value).padStart(2, '0');
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = total % 60;
  return hours > 0 ? `${hours}:${pad(minutes)}:${pad(rest)}` : `${pad(minutes)}:${pad(rest)}`;
}

/**
 * The global Simple Mode switch, shared by the web top bar and the desktop head.
 *
 * A real checkbox keeps keyboard and screen-reader behaviour for free, and the
 * wrapping label makes the whole control clickable. That wrapper matters inside
 * the desktop drag region: the drag handler skips labels and inputs, so clicking
 * the switch never starts dragging the window.
 */
export function SimpleModeSwitch({
  on,
  label,
  hint,
  onChange
}: {
  on: boolean;
  label: string;
  hint: string;
  onChange: (next: boolean) => void;
}) {
  return (
    <label className="simple-switch" title={hint}>
      <input type="checkbox" checked={on} onChange={(event) => onChange(event.target.checked)} />
      <span className="simple-switch-track" aria-hidden="true">
        <span className="simple-switch-thumb" />
      </span>
      <span className="simple-switch-label">{label}</span>
    </label>
  );
}

export function App() {
  const [mode, setMode] = useState<UiMode>('paper');
  const [lang, setLang] = useState<Lang>('zh');
  const [simpleMode, setSimpleMode] = useState(simpleModePreference);
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [toast, setToast] = useState<{ id: number; text: string; tone: 'info' | 'error' } | null>(null);
  const [paperJob, setPaperJob] = useState<JobRecord | null>(null);
  const [pptJob, setPptJob] = useState<JobRecord | null>(null);
  const [paperState, setPaperState] = useState<PaperFigureState>(defaultPaperState);
  const [pptState, setPptState] = useState<PptSlideState>(defaultPptState);
  const t = copy[lang];

  const showMessage = useMemo(() => {
    return (text: string, tone: 'info' | 'error' = 'info') => {
      setToast({ id: Date.now(), text, tone });
    };
  }, []);

  useEffect(() => {
    getConfig().then(setConfig).catch((error) => showMessage(error.message, 'error'));
  }, [showMessage]);

  useJobPolling(paperJob, setPaperJob, (message) => showMessage(message, 'error'));
  useJobPolling(pptJob, setPptJob, (message) => showMessage(message, 'error'));

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3200);
    return () => window.clearTimeout(timer);
  }, [toast]);

  async function persistConfig(next: AppConfig) {
    try {
      showMessage(t.common.saving);
      const saved = await saveConfig(next);
      setConfig(saved);
      showMessage(t.common.saved);
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Save failed', 'error');
    }
  }

  return (
    <main className="shell">
      <header className="topbar clay-panel">
        <div className="brand">
          <div className="brand-mark"><img src="/favor.png" alt="" /></div>
          <div className="brand-text">
            <h1>DREAMPAPER</h1>
          </div>
        </div>
        <div className="top-row">
          <nav className="nav" aria-label="Primary">
            <button type="button" className={mode === 'paper' ? 'active' : ''} onClick={() => setMode('paper')}>{t.nav.paper}</button>
            <button type="button" className={mode === 'ppt' ? 'active' : ''} onClick={() => setMode('ppt')}>{t.nav.ppt}</button>
          </nav>
          <div className="top-tools">
            <SimpleModeSwitch
              on={simpleMode}
              label={t.simple.label}
              hint={t.simple.hint}
              onChange={(next) => {
                saveSimpleMode(next);
                setSimpleMode(next);
              }}
            />
            <button
              type="button"
              className={`tool-button tool-icon-only ${mode === 'settings' ? 'active' : ''}`}
              onClick={() => setMode('settings')}
              aria-label={t.nav.settings}
              title={t.nav.settings}
            >
              <span className="tool-icon" aria-hidden="true">⚙</span>
            </button>
            <button type="button" className="lang-toggle" onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}>{lang === 'zh' ? 'EN' : '中'}</button>
          </div>
        </div>
      </header>
      <section className="workspace">
        {mode === 'settings' && (
          <Settings config={config} onChange={setConfig} onSave={persistConfig} t={t} />
        )}
        {mode === 'paper' && (
          <>
            <PaperFigure simpleMode={simpleMode} state={paperState} onState={setPaperState} onJob={setPaperJob} onMessage={showMessage} t={t} />
            {paperJob && <JobPanel job={paperJob} t={t} onError={(message) => showMessage(message, 'error')} />}
          </>
        )}
        {mode === 'ppt' && (
          <>
            <PptSlide simpleMode={simpleMode} state={pptState} onState={setPptState} onJob={setPptJob} onMessage={showMessage} t={t} />
            {pptJob && <JobPanel job={pptJob} t={t} onError={(message) => showMessage(message, 'error')} />}
          </>
        )}
      </section>
      {toast && (
        <div className={`toast toast-${toast.tone}`} role="status" aria-live="polite" key={toast.id}>
          <span>{toast.text}</span>
          <button type="button" className="toast-close" aria-label="Close" onClick={() => setToast(null)}>
            ×
          </button>
        </div>
      )}
    </main>
  );
}

/** 搜索协议各自的默认地址，只用来做输入框提示；留空表示没有约定。 */
export const SEARCH_PROTOCOL_BASE_URL: Record<string, string> = {
  tavily: 'https://api.tavily.com'
};

/** 一个字段都不用填的协议：端点写死在代码里，免密钥，所以 URL、密钥都不显示。 */
export function searchNeedsNoSetup(protocol: string): boolean {
  return protocol === 'duckduckgo';
}

/** 只有对话式联网搜索用得到模型；其余协议填了也不会读。 */
export function searchUsesModel(protocol: string): boolean {
  return protocol !== 'duckduckgo' && protocol !== 'tavily';
}

/** 搜索角色的当前档案：按 active_search_profile 取，找不到退回任意一份。 */
export function activeSearchProfile(config: AppConfig): ModelProfile {
  const profiles = config.model_profiles.filter((item) => item.role === 'search');
  return profiles.find((item) => item.id === config.active_search_profile) || profiles[0] || defaultSearch();
}

/**
 * 切换搜索协议：每个协议一份档案，已有的切过去，没有的补一份干净的默认档案。
 *
 * 以前只有一个搜索档案，切协议会把上一个协议的地址、模型、密钥留在原地——
 * duckduckgo 会拿着别人的地址去抓别人站点，tavily 会把别家密钥发给自己的
 * 接口。分开存之后每个协议只看得见自己的那份。
 */
export function withSearchProtocol(config: AppConfig, protocol: string): AppConfig {
  const existing = config.model_profiles.find((item) => item.role === 'search' && item.protocol === protocol);
  if (existing) return { ...config, active_search_profile: existing.id };
  const used = new Set(config.model_profiles.map((item) => item.id));
  let id = `search-${protocol}`;
  for (let index = 2; used.has(id); index += 1) id = `search-${protocol}-${index}`;
  const profile: ModelProfile = {
    ...defaultSearch(),
    id,
    name: `Search model (${protocol})`,
    protocol,
    base_url: '',
    model: '',
    api_key: null,
    has_api_key: false,
    api_key_hint: null
  };
  return { ...config, model_profiles: [...config.model_profiles, profile], active_search_profile: id };
}

const PROTOCOL_LABELS: Record<string, string> = {
  openai_responses: 'OpenAI Responses',
  openai_chat: 'OpenAI Chat',
  anthropic_messages: 'Anthropic Messages',
  image2: 'image2',
  banana2: 'banana2'
};

function protocolLabel(value: string) {
  return PROTOCOL_LABELS[value] || value;
}

export function Settings({
  config,
  onChange,
  onSave,
  t,
  tail
}: {
  config: AppConfig;
  onChange: (config: AppConfig) => void;
  onSave: (config: AppConfig) => void;
  t: typeof copy[Lang];
  /** Extra collapsible rows rendered after the built-in ones (desktop-only sections). */
  tail?: React.ReactNode;
}) {
  const design = config.model_profiles.find((item) => item.role === 'design') || defaultDesign();
  const implement = config.model_profiles.find((item) => item.role === 'implement') || defaultImplement();
  const search = activeSearchProfile(config);
  const [open, setOpen] = useState<string[]>(['design']);

  function toggle(key: string) {
    setOpen((current) => (current.includes(key) ? current.filter((item) => item !== key) : [...current, key]));
  }

  function upsert(profile: ModelProfile) {
    const rest = config.model_profiles.filter((item) => item.id !== profile.id);
    const next = { ...config, model_profiles: [...rest, profile] };
    if (profile.role === 'design') next.active_design_profile = profile.id;
    if (profile.role === 'implement') next.active_implement_profile = profile.id;
    if (profile.role === 'search') next.active_search_profile = profile.id;
    onChange(next);
  }

  function patchConfig(values: Partial<AppConfig>) {
    onChange({ ...config, ...values });
  }

  function patchConcurrency(key: 'ppt_page_plan_concurrency' | 'ppt_image_concurrency', value: string) {
    const trimmed = value.trim();
    const nextValue = trimmed ? Math.min(20, Math.max(1, Number.parseInt(trimmed, 10) || 1)) : null;
    patchConfig({ [key]: nextValue } as Partial<AppConfig>);
  }

  function modelSummary(profile: ModelProfile) {
    const parts = [protocolLabel(profile.protocol)];
    if (profile.role !== 'search' || searchUsesModel(profile.protocol)) parts.push(profile.model);
    return parts.filter(Boolean).join(' · ');
  }

  return (
    <div className="settings-stack">
      <SettingsSection
        title={t.settings.design}
        summary={modelSummary(design)}
        badge={design.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={design.has_api_key ? 'ok' : 'warn'}
        open={open.includes('design')}
        onToggle={() => toggle('design')}
      >
        <ModelEditor profile={design} onChange={upsert} t={t} />
      </SettingsSection>

      <SettingsSection
        title={t.settings.implement}
        summary={modelSummary(implement)}
        badge={implement.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={implement.has_api_key ? 'ok' : 'warn'}
        open={open.includes('implement')}
        onToggle={() => toggle('implement')}
      >
        <ModelEditor profile={implement} onChange={upsert} t={t} />
      </SettingsSection>

      <SettingsSection
        title={t.settings.search}
        summary={modelSummary(search)}
        badge={searchNeedsNoSetup(search.protocol) ? undefined : search.has_api_key ? t.settings.keySet : t.settings.keyNone}
        badgeTone={search.has_api_key ? 'ok' : 'warn'}
        open={open.includes('search')}
        onToggle={() => toggle('search')}
      >
        <ModelEditor
          profile={search}
          onChange={upsert}
          onProtocolChange={(protocol) => onChange(withSearchProtocol(config, protocol))}
          t={t}
          hint={t.settings.searchHint}
        />
      </SettingsSection>

      <SettingsSection
        title={t.settings.proxyAndConcurrency}
        summary={config.proxy_url || t.settings.notSet}
        badge={config.proxy_url ? t.settings.proxySet : t.settings.proxyNone}
        badgeTone={config.proxy_url ? 'ok' : 'warn'}
        open={open.includes('runtime')}
        onToggle={() => toggle('runtime')}
      >
        <div className="settings-side-block">
          <Field label={t.settings.proxy}>
            <input
              value={config.proxy_url ?? ''}
              placeholder="http://127.0.0.1:7890"
              onChange={(event) => patchConfig({ proxy_url: event.target.value })}
            />
          </Field>
          <p className="settings-side-note">{t.settings.proxyHint}</p>
        </div>
        <div className="settings-side-divider" aria-hidden="true" />
        <div className="settings-side-block">
          <h3 className="settings-side-subtitle">{t.settings.concurrency}</h3>
          <div className="field-row two">
            <Field label={t.settings.pagePlanConcurrency}>
              <input
                type="number"
                min="1"
                max="20"
                placeholder={t.settings.defaultByPages}
                value={config.ppt_page_plan_concurrency ?? ''}
                onChange={(event) => patchConcurrency('ppt_page_plan_concurrency', event.target.value)}
              />
            </Field>
            <Field label={t.settings.imageConcurrency}>
              <input
                type="number"
                min="1"
                max="20"
                placeholder={t.settings.defaultByPages}
                value={config.ppt_image_concurrency ?? ''}
                onChange={(event) => patchConcurrency('ppt_image_concurrency', event.target.value)}
              />
            </Field>
          </div>
          <p className="settings-side-note">{t.settings.concurrencyHint}</p>
        </div>
      </SettingsSection>

      {tail}

      <div className="actions-row left settings-actions">
        <button className="primary" onClick={() => onSave(config)}>{t.common.save}</button>
      </div>
    </div>
  );
}

export function SettingsSection({
  title,
  summary,
  badge,
  badgeTone,
  open,
  onToggle,
  children
}: {
  title: string;
  summary: string;
  badge?: string;
  badgeTone?: 'ok' | 'warn';
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
}) {
  return (
    <section className={`clay-panel settings-section${open ? ' open' : ''}`}>
      <button type="button" className="settings-section-header" onClick={onToggle} aria-expanded={open}>
        <span className="settings-section-title">{title}</span>
        <span className="settings-section-summary" title={summary}>{summary}</span>
        {badge ? <span className={`settings-section-badge ${badgeTone || 'ok'}`}>{badge}</span> : null}
        <span className="settings-section-chevron" aria-hidden="true">›</span>
      </button>
      <div className="settings-section-panel">
        <div className="settings-section-panel-inner">
          <div className="settings-section-body" inert={!open}>{children}</div>
        </div>
      </div>
    </section>
  );
}

function ModelEditor({
  profile,
  onChange,
  onProtocolChange,
  t,
  hint
}: {
  profile: ModelProfile;
  onChange: (profile: ModelProfile) => void;
  /** 搜索角色专用：切协议换的是另一份档案，不是改这一个的协议字段。 */
  onProtocolChange?: (protocol: string) => void;
  t: typeof copy[Lang];
  hint?: string;
}) {
  const isImplement = profile.role === 'implement';
  const isSearch = profile.role === 'search';
  const isDesign = profile.role === 'design';
  const defaults = profile.output_defaults || {};
  function patch(values: Partial<ModelProfile>) {
    onChange({ ...profile, ...values });
  }
  function patchDefaults(values: Record<string, string>) {
    patch({ output_defaults: { ...defaults, ...values } });
  }
  // Most defaults are strings, but the map also carries non-strings such as the
  // Grok `tools` array, which a select must not be handed.
  function defaultText(key: string, fallback: string) {
    const value = defaults[key];
    return typeof value === 'string' && value ? value : fallback;
  }
  return (
    <div className="model-fields">
      {hint && <p className="model-panel-hint">{hint}</p>}
      <Field label={t.common.protocol}>
        <select
          value={profile.protocol}
          onChange={(event) =>
            onProtocolChange ? onProtocolChange(event.target.value) : patch({ protocol: event.target.value })
          }
        >
          {profile.role === 'design' ? (
            <>
              <option value="openai_responses">OpenAI Responses</option>
              <option value="openai_chat">OpenAI Chat</option>
              <option value="anthropic_messages">Anthropic Messages</option>
            </>
          ) : profile.role === 'search' ? (
            <>
              <option value="duckduckgo">duckduckgo</option>
              <option value="tavily">tavily</option>
              <option value="grok_search">grok_search</option>
            </>
          ) : (
            <>
              <option value="image2">image2</option>
              <option value="banana2">banana2</option>
            </>
          )}
        </select>
      </Field>
      {(!isSearch || !searchNeedsNoSetup(profile.protocol)) && (
        <Field label={t.common.baseUrl}>
          <input
            value={profile.base_url}
            placeholder={isSearch ? SEARCH_PROTOCOL_BASE_URL[profile.protocol] : undefined}
            onChange={(event) => patch({ base_url: event.target.value })}
          />
        </Field>
      )}
      {(!isSearch || searchUsesModel(profile.protocol)) && (
        <Field label={t.common.model}>
          <input
            value={profile.model}
            placeholder={isSearch ? 'grok-build-0.1' : undefined}
            onChange={(event) => patch({ model: event.target.value })}
          />
        </Field>
      )}
      {(!isSearch || !searchNeedsNoSetup(profile.protocol)) && (
        <Field label={t.common.apiKey}>
          <input
            type="password"
            placeholder={profile.api_key_hint || t.common.redacted}
            onChange={(event) => patch({ api_key: event.target.value })}
          />
        </Field>
      )}
      <div className="field-row">
        <Field label={t.settings.timeout} hint={isImplement ? t.settings.timeoutHint : undefined}>
          <IntegerInput
            value={profile.timeout_seconds}
            min={5}
            max={1800}
            fallback={isImplement ? 600 : isSearch ? 20 : 120}
            onCommit={(next) => patch({ timeout_seconds: next })}
          />
        </Field>
        <Field label={t.settings.retries}>
          <IntegerInput
            value={profile.max_retries}
            min={0}
            max={8}
            fallback={0}
            onCommit={(next) => patch({ max_retries: next })}
          />
        </Field>
        {isDesign && (
          <Field label={t.settings.stream}>
            <label className="switch-field">
              <input
                type="checkbox"
                checked={defaults.stream === 'true'}
                onChange={(event) => patchDefaults({ stream: event.target.checked ? 'true' : 'false' })}
              />
              <span>{t.settings.streamOn}</span>
            </label>
          </Field>
        )}
      </div>
      {isSearch && (
        <Field label={t.settings.maxResults}>
          <IntegerInput
            value={Number.parseInt(defaultText('max_results', '3'), 10) || 3}
            min={1}
            max={8}
            fallback={3}
            onCommit={(next) => patchDefaults({ max_results: String(next) })}
          />
        </Field>
      )}
      {isImplement && profile.protocol === 'banana2' && (
        <Field label={t.settings.version}>
          <input value={profile.api_version || 'v1beta'} onChange={(event) => patch({ api_version: event.target.value })} />
        </Field>
      )}
      {isImplement && profile.protocol === 'image2' && (
        <div className="field-row four">
          <Field label={t.settings.size}>
            <select value={defaultText('size', '1200x675')} onChange={(event) => patchDefaults({ size: event.target.value })}>
              <option value="auto">auto</option>
              <option value="16:9">16:9</option>
              <option value="1024x1024">1024x1024</option>
              <option value="1200x675">1200x675</option>
              <option value="928x1664">928x1664</option>
              <option value="3000x1000">3000x1000</option>
            </select>
          </Field>
          <Field label={t.settings.quality}>
            <select value={defaultText('quality', 'auto')} onChange={(event) => patchDefaults({ quality: event.target.value })}>
              <option value="auto">auto</option>
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
              <option value="hd">hd</option>
            </select>
          </Field>
          <Field label={t.settings.format}>
            <select value={defaultText('output_format', 'png')} onChange={(event) => patchDefaults({ output_format: event.target.value })}>
              <option value="png">png</option>
              <option value="jpeg">jpeg</option>
              <option value="webp">webp</option>
            </select>
          </Field>
          <Field label="response">
            <select value={defaultText('response_format', 'url')} onChange={(event) => patchDefaults({ response_format: event.target.value })}>
              <option value="url">url</option>
              <option value="b64_json">b64_json</option>
            </select>
          </Field>
        </div>
      )}
      {isImplement && profile.protocol === 'banana2' && (
        <div className="field-row four">
          <Field label={t.settings.ratio}>
            <select value={defaultText('aspect_ratio', '16:9')} onChange={(event) => patchDefaults({ aspect_ratio: event.target.value })}>
              <option value="16:9">16:9</option>
              <option value="4:3">4:3</option>
              <option value="1:1">1:1</option>
              <option value="3:2">3:2</option>
            </select>
          </Field>
          <Field label={t.settings.clarity}>
            <select value={defaultText('image_size', '4K')} onChange={(event) => patchDefaults({ image_size: event.target.value })}>
              <option value="1K">1K</option>
              <option value="2K">2K</option>
              <option value="4K">4K</option>
            </select>
          </Field>
          <Field label={t.settings.tendency}>
            <select value={defaultText('thinking_level', 'high')} onChange={(event) => patchDefaults({ thinking_level: event.target.value })}>
              <option value="minimal">minimal</option>
              <option value="high">high</option>
            </select>
          </Field>
          <Field label={t.settings.format}>
            <select value={defaultText('mime_type', 'image/png')} onChange={(event) => patchDefaults({ mime_type: event.target.value })}>
              <option value="image/png">image/png</option>
              <option value="image/jpeg">image/jpeg</option>
              <option value="image/webp">image/webp</option>
            </select>
          </Field>
        </div>
      )}
    </div>
  );
}

export function PaperFigure({ simpleMode, state, onState, onJob, onMessage, t }: { simpleMode: boolean; state: PaperFigureState; onState: StateUpdater<PaperFigureState>; onJob: (job: JobRecord) => void; onMessage: (message: string, tone?: 'info' | 'error') => void; t: typeof copy[Lang] }) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const formBodyRef = useRef<HTMLDivElement>(null);
  const galleryRef = useRef<HTMLDivElement>(null);
  const { kind, query, selected, title, description, aspectRatio, layoutFidelity, styleStrength, custom } = state;

  function patch(values: Partial<PaperFigureState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    listTemplates(kind, query).then(setTemplates).catch(console.error);
  }, [kind, query]);

  useEffect(() => {
    const form = formBodyRef.current;
    const gallery = galleryRef.current;
    if (!form || !gallery || typeof ResizeObserver === 'undefined') return;

    const syncHeight = () => {
      const next = Math.round(form.getBoundingClientRect().height);
      if (next > 0) gallery.style.height = `${next}px`;
    };

    syncHeight();
    const observer = new ResizeObserver(syncHeight);
    observer.observe(form);
    window.addEventListener('resize', syncHeight);
    return () => {
      observer.disconnect();
      window.removeEventListener('resize', syncHeight);
      gallery.style.height = '';
    };
  }, []);

  function toggle(id: string) {
    onState((current) => ({
      ...current,
      selected: current.selected.includes(id) ? current.selected.filter((item) => item !== id) : [...current.selected, id].slice(0, 3)
    }));
  }

  async function submit() {
    try {
      onMessage(t.paper.submitted);
      const job = await createJob({
        mode: 'paper_figure',
        simple_mode: simpleMode,
        payload: {
          figure_title: title,
          section_description: description,
          template_ids: selected,
          aspect_ratio: aspectRatio,
          layout_fidelity: layoutFidelity,
          style_strength: styleStrength,
          candidate_count: 1,
          custom_prompt: custom || null
        }
      });
      onJob(job);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.paper.title} text={t.paper.intro} />
      <div className="figure-layout">
        <div className="form-panel workflow-form figure-form-body" ref={formBodyRef}>
          <Field label={t.paper.figureTitle}>
            <input value={title} onChange={(event) => patch({ title: event.target.value })} />
          </Field>
          <Field label={t.paper.description}>
            <textarea className="figure-method-textarea" value={description} onChange={(event) => patch({ description: event.target.value })} />
          </Field>
          <div className="field-row three">
            <Field label={t.paper.ratio}>
              <select value={aspectRatio} onChange={(event) => patch({ aspectRatio: event.target.value })}>
                <option value="inherit">{t.paper.inherited}</option>
                <option value="16:9">16:9</option>
                <option value="4:3">4:3</option>
                <option value="1:1">1:1</option>
                <option value="3:2">3:2</option>
              </select>
            </Field>
            <Field label={t.paper.fidelity}>
              <select value={layoutFidelity} onChange={(event) => patch({ layoutFidelity: event.target.value as PaperFigureState['layoutFidelity'] })}>
                <option value="strict">strict</option>
                <option value="balanced">balanced</option>
                <option value="loose">loose</option>
              </select>
            </Field>
            <Field label={t.paper.strength}>
              <select value={styleStrength} onChange={(event) => patch({ styleStrength: event.target.value as PaperFigureState['styleStrength'] })}>
                <option value="high">high</option>
                <option value="medium">medium</option>
                <option value="low">low</option>
              </select>
            </Field>
          </div>
          <Field label={t.paper.custom} hint={t.paper.customHint}>
            <textarea className="figure-custom-textarea" value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
        </div>
        <div className="gallery-panel" ref={galleryRef}>
          <div className="toolbar field-row two">
            <Field label={t.paper.kind}>
              <select value={kind} onChange={(event) => patch({ kind: event.target.value })}>
                <option value="diagram">diagram</option>
                <option value="plot">plot</option>
              </select>
            </Field>
            <Field label={t.paper.search}>
              <input placeholder={t.paper.search} value={query} onChange={(event) => patch({ query: event.target.value })} />
            </Field>
          </div>
          <div className="template-grid">
            {templates.map((template) => (
              <button key={template.id} className={`template-tile ${selected.includes(template.id) ? 'selected' : ''}`} onClick={() => toggle(template.id)}>
                <img src={template.image_url} alt="" />
                <span>{template.category || template.kind}</span>
              </button>
            ))}
          </div>
        </div>
        <div className="actions-row left figure-actions">
          <button className="primary" disabled={!title || !description || selected.length === 0} onClick={submit}>{t.paper.generate}</button>
        </div>
      </div>
    </section>
  );
}

export function PptSlide({ simpleMode, state, onState, onJob, onMessage, t }: { simpleMode: boolean; state: PptSlideState; onState: StateUpdater<PptSlideState>; onJob: (job: JobRecord) => void; onMessage: (message: string, tone?: 'info' | 'error') => void; t: typeof copy[Lang] }) {
  const { asset, materials, material, pages, custom } = state;

  function patch(values: Partial<PptSlideState>) {
    onState((current) => ({ ...current, ...values }));
  }

  async function onTemplateFile(file?: File) {
    if (!file) return;
    try {
      onMessage(t.ppt.uploading);
      const uploaded = await uploadAsset(file);
      patch({ asset: uploaded });
      onMessage(t.ppt.uploaded);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed, 'error');
    }
  }

  async function onMaterialFiles(files?: FileList | null) {
    if (!files?.length) return;
    try {
      onMessage(t.ppt.uploading);
      const uploaded = await Promise.all(Array.from(files).map((file) => uploadAsset(file)));
      onState((current) => ({ ...current, materials: [...current.materials, ...uploaded].slice(0, 10) }));
      onMessage(t.ppt.uploaded);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.uploadFailed, 'error');
    }
  }

  async function submit() {
    if (!asset) return;
    try {
      onMessage(t.ppt.submitted);
      const job = await createJob({
        mode: 'ppt_slide',
        simple_mode: simpleMode,
        payload: {
          template_asset_id: asset.id,
          material_text: material,
          material_asset_ids: materials.map((item) => item.id),
          page_count: pages,
          custom_prompt: custom || null
        }
      });
      onJob(job);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  return (
    <section className="clay-panel page-panel">
      <Header title={t.ppt.title} text={t.ppt.intro} />
      <div className="slide-layout">
        <div className="slide-master-head">
          <Field label={t.ppt.template}>
            <FilePicker
              compact
              accept="image/*"
              label={t.common.choose}
              value={asset?.filename || ''}
              onChange={(files) => onTemplateFile(files?.[0])}
            />
          </Field>
        </div>
        <div className="slide-meta-head">
          <Field label={t.ppt.pages}>
            <input
              className="pages-input"
              type="number"
              min={1}
              max={20}
              value={pages}
              onChange={(event) => patch({ pages: Number(event.target.value) })}
            />
          </Field>
          <Field label={t.ppt.materialFile}>
            <FilePicker
              compact
              accept=".pdf,.doc,.docx,.txt,.md,.markdown,.csv,.tsv,.json,application/pdf,text/*"
              label={t.common.choose}
              value={materials.length ? `${t.common.uploadedFiles} ${materials.length}` : ''}
              multiple
              onChange={onMaterialFiles}
            />
          </Field>
        </div>

        <div className={`slide-preview-wrap ${asset ? 'has-asset' : ''}`}>
          {asset ? (
            <div className="template-preview-frame has-image">
              <img className="template-preview" src={asset.url} alt="" />
            </div>
          ) : (
            <div className="template-preview-frame">
              <span className="template-preview-empty">{t.common.noFile}</span>
            </div>
          )}
        </div>

        <div className="slide-material-wrap">
          {materials.length > 0 && (
            <div className="file-list">
              {materials.map((item) => <span key={item.id}>{item.filename}</span>)}
            </div>
          )}
          <Field label={t.ppt.material} hint={t.ppt.materialHint}>
            <textarea className="material-textarea" value={material} onChange={(event) => patch({ material: event.target.value })} />
          </Field>
        </div>

        <div className="slide-custom-wrap">
          <Field label={t.ppt.custom} hint={t.ppt.customHint}>
            <textarea className="custom-textarea" value={custom} onChange={(event) => patch({ custom: event.target.value })} />
          </Field>
        </div>

        <div className="actions-row right slide-actions">
          <button className="primary" disabled={!asset || (!material.trim() && materials.length === 0)} onClick={submit}>{t.ppt.generate}</button>
        </div>
      </div>
    </section>
  );
}

export function JobPanel({
  job,
  t,
  onCancelled,
  onResumed,
  onError,
  onOpenWorkbench,
  onRated
}: {
  job: JobRecord;
  t: typeof copy[Lang];
  onCancelled?: (job: JobRecord) => void;
  /** 失败或已停止的任务重跑：同一条记录，已完成的部分从缓存回放。 */
  onResumed?: (job: JobRecord) => void;
  onError?: (message: string) => void;
  onOpenWorkbench?: (assetId: string) => void;
  /** Receives the refreshed record after a 优/良/差 tag is saved. */
  onRated?: (job: JobRecord) => void;
}) {
  const images = useMemo(() => job.images || [], [job.images]);
  const events = job.events || [];
  const latest = events.length > 0 ? events[events.length - 1] : null;
  const displayMessage = job.error?.summary?.trim() || latest?.message?.trim() || job.message?.trim() || statusText(job.status, t);
  const displayStage = job.error?.stage || latest?.stage || job.stage || job.status;
  const progress = useMemo(() => computeJobProgress(job), [job]);
  const [stepAnimKey, setStepAnimKey] = useState(0);
  const [stopping, setStopping] = useState(false);
  const [resuming, setResuming] = useState(false);
  const designCards = useDesignLogs(job);
  const finalPrompts = useFinalPrompts(job);
  const searchTraces = searchRounds(useSearchTraces(job));
  const flowEntries = useMemo(
    () => buildFlowEntries(designCards, searchTraces, finalPrompts, t, job.mode),
    [designCards, searchTraces, finalPrompts, t, job.mode]
  );
  // Which rows the reader opened, plus the steps that are still streaming: a run
  // in progress should read as blocks appearing one after another.
  const [openKeys, setOpenKeys] = useState<string[]>([]);
  const seenLiveKeys = useRef(new Set<string>());
  const searchEntries = flowEntries.filter((entry) => entry.phase === 'search');
  const designEntries = flowEntries.filter((entry) => entry.phase === 'design');
  const promptEntries = flowEntries.filter((entry) => entry.phase === 'implement');
  const simpleMode = job.payload?.simple_mode ?? promptEntries.length > 0;
  // 卡片只展示任务真正进入的阶段：检索和设计看有没有输出，制图看是否已开工。
  const stages = useMemo(
    () =>
      [
        { phase: 'search', title: t.result.searchStage, hint: t.result.searchHint, entries: searchEntries },
        { phase: 'design', title: t.result.designStage, hint: t.result.designHint, entries: designEntries }
      ].filter((stage) => stage.entries.length > 0),
    [searchEntries, designEntries, t]
  );
  const implementReached = simpleMode ? promptEntries.length > 0 : images.length > 0 || implementStarted(job);
  const toggleFlow = useCallback((key: string) => {
    setOpenKeys((keys) => (keys.includes(key) ? keys.filter((current) => current !== key) : [...keys, key]));
  }, []);
  const [copied, setCopied] = useState<{ jobId: string; step: string } | null>(null);
  const elapsed = useElapsedSeconds(job);
  const settled = isJobSettled(job.status);
  const canStop = Boolean(onCancelled) && desktopAvailable() && !settled;
  // 继续/重试只对已经停下的失败或已停止任务有意义：任务还在跑时按钮就是“停止”。
  const canResume = Boolean(onResumed) && desktopAvailable() && settled && job.status !== 'succeeded';
  const [rating, setRating] = useState(false);
  const canRate = Boolean(onRated) && desktopAvailable() && job.status === 'succeeded';

  async function rate(next: JobRating) {
    if (!onRated || rating) return;
    setRating(true);
    try {
      // Clicking the active tag clears it.
      onRated(await rateJob(job.id, job.rating === next ? null : next));
    } catch (error) {
      onError?.(error instanceof Error ? error.message : t.result.rateFailed);
    } finally {
      setRating(false);
    }
  }

  useEffect(() => {
    setStepAnimKey((key) => key + 1);
  }, [displayStage, displayMessage, latest?.timestamp]);

  useEffect(() => {
    setStopping(false);
    setResuming(false);
  }, [job.id, settled]);

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(null), 1600);
    return () => window.clearTimeout(timer);
  }, [copied]);

  async function copyPrompt(entry: FlowEntry) {
    if (!entry.text) return;
    try {
      await navigator.clipboard.writeText(entry.text);
      // 带上任务 id：切换任务后同名的 final_prompt_1 不会显示成“已复制”。
      setCopied({ jobId: job.id, step: entry.step });
    } catch {
      // No clipboard (insecure context) or a denied permission: the text is
      // still selectable by hand, so this is a message and not a failure.
      onError?.(t.result.copyFailed);
    }
  }

  // Process details default to collapsed; final prompts default to expanded.
  // Expansion and live auto-open bookkeeping belong to this job only.
  useEffect(() => {
    setOpenKeys([]);
    seenLiveKeys.current.clear();
  }, [job.id]);

  useEffect(() => {
    const live = designCards
      .filter((card) => card.status === 'running' && !settled && !seenLiveKeys.current.has(card.step))
      .map((card) => card.step);
    live.forEach((step) => seenLiveKeys.current.add(step));
    if (live.length === 0) return;
    setOpenKeys((keys) => {
      const missing = live.filter((step) => !keys.includes(step));
      return missing.length === 0 ? keys : [...keys, ...missing];
    });
  }, [designCards, settled]);

  async function stop() {
    if (!onCancelled) return;
    setStopping(true);
    try {
      onCancelled(await cancelJob(job.id));
    } catch (error) {
      setStopping(false);
      onError?.(error instanceof Error ? error.message : t.result.stopFailed);
    }
  }

  async function resume() {
    if (!onResumed) return;
    setResuming(true);
    try {
      onResumed(await resumeJob(job.id));
    } catch (error) {
      setResuming(false);
      onError?.(error instanceof Error ? error.message : t.result.resumeFailed);
    }
  }

  return (
    <section className="clay-panel result-panel">
      <Header title={t.result.title} text={`${statusText(job.status, t)} · ${progress}%`} />
      <div className={`progress-card progress-card-live ${job.status}`}>
        <div className="progress-main-row">
          <CircularProgress value={progress} status={job.status} />
          <div className="progress-main-body">
            <div className="progress-meta-row">
              <span className="progress-label">{t.result.progress}</span>
              <strong className="progress-percent">{progress}%</strong>
              <span className={`status-pill ${job.status}`}>{statusText(job.status, t)}</span>
              <span className="progress-elapsed" title={t.result.elapsed}>
                <span className="progress-elapsed-label">{t.result.elapsed}</span>
                <time dateTime={`PT${elapsed}S`}>{formatDuration(elapsed)}</time>
              </span>
              <span className="progress-step-count">
                {events.length > 0 ? `${events.length}` : '0'}
              </span>
              {canStop && (
                <button type="button" className="stop-job-button" disabled={stopping} onClick={stop}>
                  {stopping ? t.result.stopping : t.result.stop}
                </button>
              )}
              {canResume && (
                <button type="button" className="resume-job-button" disabled={resuming} onClick={resume}>
                  {resuming ? t.result.resuming : job.status === 'cancelled' ? t.result.resume : t.result.retry}
                </button>
              )}
            </div>
            <div className={`progress-track ${job.status}`} role="progressbar" aria-valuenow={progress} aria-valuemin={0} aria-valuemax={100}>
              <span style={{ width: `${progress}%` }} />
            </div>
            <div className="step-log" aria-live="polite">
              <span className="step-log-label">{t.result.step}</span>
              <div key={stepAnimKey} className={`step-log-panel ${latest?.status || job.status}`}>
                <div className="step-log-dot" />
                <div className="step-log-text">
                  <strong>{displayMessage}</strong>
                  <small title={displayStage}>{stageLabel(displayStage, t)}</small>
                </div>
              </div>
            </div>
            {job.status === 'failed' && job.error && (
              <div className="error-diagnostic" aria-label={t.result.errorTitle}>
                <h3>{t.result.errorTitle}</h3>
                <dl>
                  {job.error.role && <><dt>{t.result.errorRole}</dt><dd>{job.error.role.toUpperCase()}</dd></>}
                  {(job.error.profile_name || job.error.profile_id) && <><dt>{t.result.errorProfile}</dt><dd>{job.error.profile_name || job.error.profile_id}</dd></>}
                  {job.error.model && <><dt>{t.result.errorModel}</dt><dd>{job.error.model}</dd></>}
                  {job.error.http_status && <><dt>{t.result.errorStatus}</dt><dd>{job.error.http_status}</dd></>}
                  {job.error.stage && <><dt>{t.result.errorStage}</dt><dd title={job.error.stage}>{stageLabel(job.error.stage, t)}</dd></>}
                  {(job.error.endpoint || job.error.base_url) && <><dt>{t.result.errorEndpoint}</dt><dd>{job.error.endpoint || job.error.base_url}</dd></>}
                  {job.error.suggestion && <><dt>{t.result.errorSuggestion}</dt><dd>{job.error.suggestion}</dd></>}
                </dl>
              </div>
            )}
          </div>
        </div>
      </div>
      <section className="flow-log" aria-label={t.result.flowLog}>
        {stages.map((stage) => (
          <div key={`${job.id}:${stage.phase}`} className="flow-stage-entry">
            <div className="flow-stage-entry-inner">
              <section className={`flow-stage flow-stage-${stage.phase}`} aria-label={stage.title}>
                <div className="flow-log-head">
                  <h3>{stage.title}</h3>
                  <span className="flow-log-count">{stage.entries.length}</span>
                </div>
                <p className="flow-log-hint">{stage.hint}</p>
                <ol className="flow-list">
                  {stage.entries.map((entry) => (
                    <FlowItem
                      key={`${job.id}:${entry.key}`}
                      entry={entry}
                      open={openKeys.includes(entry.key)}
                      active={!settled && (entry.step === displayStage || entry.status === 'running')}
                      settled={settled}
                      t={t}
                      copied={false}
                      onToggle={() => toggleFlow(entry.key)}
                    />
                  ))}
                </ol>
              </section>
            </div>
          </div>
        ))}
      </section>
      {implementReached && (
        <div className="flow-stage-entry">
          <div className="flow-stage-entry-inner">
            <section className="flow-stage implement-results" aria-label={t.result.implementStage}>
              <div className="flow-log-head">
                <h3>{t.result.implementStage}</h3>
                <span className="flow-log-count">{simpleMode ? promptEntries.length : images.length}</span>
              </div>
              {simpleMode && <p className="flow-log-hint">{promptEntries.length > 0 ? t.result.finalPromptHint : t.simple.hint}</p>}
              {simpleMode && promptEntries.length > 0 && (
                <ol className="flow-list final-prompt-list">
                  {promptEntries.map((entry) => (
                    <FlowItem
                      key={`${job.id}:${entry.key}`}
                      entry={entry}
                      open={!openKeys.includes(entry.key)}
                      active={false}
                      settled={settled}
                      t={t}
                      copied={copied?.jobId === job.id && copied.step === entry.step}
                      onToggle={() => toggleFlow(entry.key)}
                      onCopy={() => void copyPrompt(entry)}
                    />
                  ))}
                </ol>
              )}
              {(simpleMode ? promptEntries.length === 0 : images.length === 0) && (
                <p className="flow-empty">{t.result.implementEmpty}</p>
              )}
              {canRate && (
                <div className="rate-row" role="group" aria-label={t.result.rate} title={t.result.rateHint}>
                  <span className="rate-label">{t.result.rate}</span>
                  {(['good', 'fair', 'poor'] as JobRating[]).map((value) => (
                    <button
                      key={value}
                      type="button"
                      className={`rate-btn rate-${value}${job.rating === value ? ' active' : ''}`}
                      aria-pressed={job.rating === value}
                      disabled={rating}
                      onClick={() => void rate(value)}
                    >
                      {value === 'good' ? t.result.rateGood : value === 'fair' ? t.result.rateFair : t.result.ratePoor}
                    </button>
                  ))}
                </div>
              )}
              {!simpleMode && images.length > 0 && (
                <div className="result-grid">
                  {images.map((image) => (
                    <div key={image.url} className="result-card">
                      <a
                        className="result-preview"
                        href={image.url}
                        target="_blank"
                        rel="noreferrer"
                        aria-label={`${t.result.preview} ${image.name}`}
                        onClick={(event) => {
                          if (!desktopAvailable()) return;
                          event.preventDefault();
                          const assetId = image.asset_id ?? image.url.split('/').pop();
                          if (!assetId) return;
                          openArtifact(assetId).catch((error) =>
                            onError?.(error instanceof Error ? error.message : t.result.previewFailed)
                          );
                        }}
                      >
                        <img src={image.url} alt={image.name} />
                      </a>
                      <div className="result-card-footer">
                        <span>{image.name}</span>
                        {/* One row, equal height: the workbench button hugs the left
                            edge and the download button the right, whatever their
                            label widths. */}
                        <div className="result-card-actions">
                          {onOpenWorkbench && image.asset_id && (
                            <button type="button" className="download-button" onClick={() => onOpenWorkbench(image.asset_id!)}>
                              {t.result.editWorkbench}
                            </button>
                          )}
                          {desktopAvailable() ? (
                            <button
                              type="button"
                              className="download-button result-download"
                              onClick={async () => {
                                const assetId = image.asset_id ?? image.url.split('/').pop();
                                if (!assetId) return;
                                try {
                                  await saveAsset(assetId, image.name);
                                } catch (error) {
                                  onError?.(error instanceof Error ? error.message : t.result.saveFailed);
                                }
                              }}
                            >
                              {t.result.download}
                            </button>
                          ) : (
                            <a className="download-button result-download" href={image.url} download={image.name}>{t.result.download}</a>
                          )}
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </section>
          </div>
        </div>
      )}
    </section>
  );
}

/**
 * One row of the generation flow.
 *
 * Search and Design use this disclosure within their own stage. Final prompts
 * reuse it in the independent Implement result section. A stable keyed wrapper
 * animates insertion even for completed rows received through polling.
 */
const FlowItem = memo(function FlowItem({
  entry,
  open,
  active,
  settled,
  t,
  copied,
  onToggle,
  onCopy
}: {
  entry: FlowEntry;
  open: boolean;
  active: boolean;
  settled: boolean;
  t: typeof copy[Lang];
  copied: boolean;
  onToggle: () => void;
  onCopy?: () => void;
}) {
  const body = useRef<HTMLPreElement>(null);
  const bodyId = useId();
  // Stopping a job aborts the pipeline task, so the step that was mid-stream
  // never reports an end. Once the job itself has settled nothing can still be
  // arriving, whatever the last row state said.
  const streaming = entry.status === 'running' && !settled;

  // Follow the text while it is still arriving. Once the step settles the view
  // stays where the reader left it, so a finished row can be scrolled back
  // through without being yanked to the bottom.
  useEffect(() => {
    if (!streaming || !open) return;
    const element = body.current;
    if (element) element.scrollTop = element.scrollHeight;
  }, [entry.text, streaming, open]);

  return (
    <li className="flow-entry">
      <div className="flow-entry-inner">
        <article className={`flow-item ${entry.kind} ${entry.status}${open ? ' is-open' : ''}${active ? ' active' : ''}`}>
          <div className="flow-head">
            <button type="button" className="flow-toggle" aria-expanded={open} aria-controls={bodyId} onClick={onToggle}>
              <span className="flow-caret" aria-hidden="true" />
              <span className="flow-dot" aria-hidden="true" />
              <span className="flow-title">{entry.title}</span>
              {streaming && <span className="flow-badge">{t.result.stepStreaming}</span>}
            </button>
            {onCopy && (
              <button type="button" className="flow-copy" onClick={onCopy}>
                {copied ? t.result.copied : t.result.copy}
              </button>
            )}
          </div>
          <div id={bodyId} className="flow-body" aria-hidden={!open} inert={!open}>
            <div className="flow-body-inner">
              {entry.kind === 'search' ? (
                <>
                  {entry.request && (
                    <div className="flow-block">
                      <span className="flow-block-label">{t.result.queryContent}</span>
                      <pre className="flow-text">{entry.request}</pre>
                    </div>
                  )}
                  {entry.response && (
                    <div className="flow-block">
                      <span className="flow-block-label">{t.result.queryResult}</span>
                      <pre className="flow-text">{entry.response}</pre>
                    </div>
                  )}
                </>
              ) : (
                // 还在接收且一个字都没到时不铺空块：空白块比“没有内容”更像出了问题。
                (entry.text || !streaming) && (
                  <div className="flow-block">
                    <pre className="flow-text" ref={body}>
                      {entry.text || t.result.stepEmpty}
                    </pre>
                  </div>
                )
              )}
            </div>
          </div>
        </article>
      </div>
    </li>
  );
});

function CircularProgress({ value, status }: { value: number; status: JobRecord['status'] }) {
  const size = 72;
  const stroke = 7;
  const radius = (size - stroke) / 2;
  const circumference = 2 * Math.PI * radius;
  const offset = circumference * (1 - Math.min(100, Math.max(0, value)) / 100);
  return (
    <div className={`circular-progress ${status}`} aria-hidden="true">
      <svg width={size} height={size} viewBox={`0 0 ${size} ${size}`}>
        <circle className="circular-progress-bg" cx={size / 2} cy={size / 2} r={radius} strokeWidth={stroke} />
        <circle
          className="circular-progress-fg"
          cx={size / 2}
          cy={size / 2}
          r={radius}
          strokeWidth={stroke}
          strokeDasharray={circumference}
          strokeDashoffset={offset}
        />
      </svg>
      <span className="circular-progress-value">{value}%</span>
    </div>
  );
}

function statusText(status: JobRecord['status'], t: typeof copy[Lang]) {
  if (status === 'queued') return t.result.queued;
  if (status === 'running') return t.result.running;
  if (status === 'succeeded') return t.result.completed;
  if (status === 'cancelled') return t.result.cancelled;
  return t.result.failed;
}

function computeJobProgress(job: JobRecord): number {
  if (job.status === 'succeeded') return 100;
  if (job.status === 'queued') return 4;

  const settledStage = job.status === 'cancelled' || job.status === 'failed';
  const lastStage = settledStage
    ? job.error?.stage || job.events?.filter((event) => !['cancelled', 'failed'].includes(event.stage)).slice(-1)[0]?.stage
    : job.stage || job.events?.[job.events.length - 1]?.stage;
  const stage = (lastStage || 'started').toLowerCase();
  const pagePlan = stage.match(/^ppt_page_(?:prompt|plan)_(\d+)$/);
  if (pagePlan) {
    const page = Number.parseInt(pagePlan[1], 10) || 1;
    return Math.min(71, 56 + Math.min(page, 12) * 1.2);
  }
  const pageImpl = stage.match(/^ppt_implement_(\d+)$/);
  if (pageImpl) {
    const page = Number.parseInt(pageImpl[1], 10) || 1;
    return Math.min(96, 76 + Math.min(page, 12) * 1.5);
  }

  const weights = stage.startsWith('paper_') ? PAPER_STAGE_WEIGHTS : stage.startsWith('ppt_') || stage in PPT_STAGE_WEIGHTS ? PPT_STAGE_WEIGHTS : { ...PAPER_STAGE_WEIGHTS, ...PPT_STAGE_WEIGHTS };
  if (stage in weights) {
    const base = weights[stage];
    if (job.status === 'failed') return Math.min(96, Math.max(8, base));
    return base;
  }

  const eventCount = job.events?.length || 1;
  const fallback = Math.min(92, 10 + eventCount * 4);
  return job.status === 'failed' ? Math.min(96, fallback) : fallback;
}

function Header({ title, text }: { title: string; text: string }) {
  return (
    <div className="section-header">
      <h2>{title}</h2>
      <p>{text}</p>
    </div>
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className={`field${hint ? ' has-hint' : ''}`}>
      <span className="field-label">{label}</span>
      <div className="field-control">{children}</div>
      {hint ? <small className="field-hint">{hint}</small> : null}
    </div>
  );
}

function IntegerInput({
  value,
  min,
  max,
  fallback,
  onCommit
}: {
  value: number;
  min: number;
  max: number;
  fallback: number;
  onCommit: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  const [focused, setFocused] = useState(false);

  useEffect(() => {
    if (!focused) setText(String(value));
  }, [value, focused]);

  function commit(raw: string) {
    const trimmed = raw.trim();
    if (trimmed === '' || !/^\d+$/.test(trimmed)) {
      onCommit(fallback);
      setText(String(fallback));
      return;
    }
    const parsed = Number.parseInt(trimmed, 10);
    const next = Math.min(max, Math.max(min, parsed));
    onCommit(next);
    setText(String(next));
  }

  return (
    <input
      type="text"
      inputMode="numeric"
      autoComplete="off"
      value={text}
      onFocus={() => setFocused(true)}
      onChange={(event) => {
        const raw = event.target.value;
        if (raw === '' || /^\d+$/.test(raw)) setText(raw);
      }}
      onBlur={() => {
        setFocused(false);
        commit(text);
      }}
      onKeyDown={(event) => {
        if (event.key === 'Enter') {
          event.currentTarget.blur();
        }
      }}
    />
  );
}

export function FilePicker({
  accept,
  label,
  value,
  multiple,
  compact,
  onChange
}: {
  accept: string;
  label: string;
  value: string;
  multiple?: boolean;
  compact?: boolean;
  onChange: (files: FileList | null) => void;
}) {
  const id = useId();
  return (
    <div className={`file-picker${compact ? ' file-picker-compact' : ''}`}>
      <input id={id} type="file" accept={accept} multiple={multiple} onChange={(event) => onChange(event.target.files)} />
      <label htmlFor={id}>{label}</label>
      {!compact && value ? <span>{value}</span> : null}
      {compact && value ? <span className="file-picker-name" title={value}>{value}</span> : null}
    </div>
  );
}

function defaultDesign(): ModelProfile {
  return { id: 'design-default', role: 'design', name: 'Design model', protocol: 'openai_responses', base_url: 'https://api.openai.com', model: 'gpt-5.4', headers: {}, timeout_seconds: 120, max_retries: 2, output_defaults: {} };
}

function defaultImplement(): ModelProfile {
  return { id: 'implement-default', role: 'implement', name: 'Implement model', protocol: 'image2', base_url: 'https://api.openai.com', model: 'gpt-image-2', headers: {}, timeout_seconds: 600, max_retries: 3, output_defaults: { size: '1200x675', quality: 'auto', output_format: 'png', response_format: 'url', aspect_ratio: '16:9', image_size: '4K', thinking_level: 'high', mime_type: 'image/png' } };
}

function defaultSearch(): ModelProfile {
  return {
    id: 'search-default',
    role: 'search',
    name: 'Search model',
    protocol: 'duckduckgo',
    base_url: '',
    model: '',
    headers: {},
    timeout_seconds: 120,
    max_retries: 1,
    output_defaults: { max_results: '3' }
  };
}
