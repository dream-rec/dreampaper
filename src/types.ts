export type ModelRole = 'design' | 'implement' | 'search';
export type Mode = 'paper_figure' | 'ppt_slide';

/** A value as stored in `output_defaults`: most keys are strings, `tools` is an array. */
export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

export interface ModelProfile {
  id: string;
  role: ModelRole;
  name: string;
  protocol: string;
  base_url: string;
  model: string;
  api_key?: string | null;
  api_version?: string | null;
  headers: Record<string, string>;
  timeout_seconds: number;
  max_retries: number;
  output_defaults: Record<string, JsonValue>;
  has_api_key?: boolean;
  api_key_hint?: string | null;
}

export interface AppConfig {
  version: number;
  active_design_profile: string;
  active_implement_profile: string;
  active_search_profile?: string;
  proxy_url?: string | null;
  ppt_page_plan_concurrency?: number | null;
  ppt_image_concurrency?: number | null;
  model_profiles: ModelProfile[];
}

export interface TemplateSummary {
  id: string;
  source_id: string;
  kind: 'diagram' | 'plot' | 'master';
  category?: string | null;
  rounded_ratio?: string | null;
  visual_intent: string;
  content_summary: string;
  image_url: string;
}

export interface TemplatePackSummary {
  id: string;
  name: string;
  version?: string | null;
  template_count: number;
}

export interface AssetUpload {
  id: string;
  filename: string;
  mime_type: string;
  url: string;
}

export interface ReleaseInfo {
  current_version: string;
  latest_version: string;
  release_url: string;
  update_available: boolean;
}

export interface JobEvent {
  stage: string;
  message: string;
  status: 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  timestamp: string;
}

export interface JobError {
  summary: string;
  code: string;
  stage?: string | null;
  role?: ModelRole | null;
  profile_id?: string | null;
  profile_name?: string | null;
  protocol?: string | null;
  model?: string | null;
  base_url?: string | null;
  endpoint?: string | null;
  http_status?: number | null;
  suggestion?: string | null;
}

export interface JobDesignLog {
  step: string;
  label: string;
  status: string;
  content: string;
  timestamp: string;
}

/**
 * The envelope `create_job` takes.
 *
 * `simple_mode` is a snapshot of the global switch at submit time, so a job
 * keeps running the mode it was started in; it is absent on records written
 * before 0.2.1 and reads back as ordinary mode.
 */
export type JobEnvelope = {
  mode: Mode;
  simple_mode?: boolean;
  payload: Record<string, unknown>;
};

export type JobCreateRequest = JobEnvelope & { simple_mode: boolean };

export interface JobRecord {
  id: string;
  mode: Mode;
  status: 'queued' | 'running' | 'succeeded' | 'failed' | 'cancelled';
  message?: string | null;
  stage?: string;
  created_at: string;
  updated_at: string;
  images: Array<{ name: string; url: string; asset_id?: string | null }>;
  events?: JobEvent[];
  error?: JobError | null;
  title?: string | null;
  thumbnail?: string | null;
  payload?: JobEnvelope | null;
  /** Only get_job carries these; list rows leave them empty. */
  design_logs?: JobDesignLog[];
  /** 优 / 良 / 差 tag on a finished job; null until the user rates it. */
  rating?: JobRating | null;
  /**
   * 简单模式：产物是制图提示词，没有成品图，历史卡片于是显示所选参考母版。
   * 桌面端从列表行就会带上；Web 后端没有这个字段。
   */
  simple?: boolean;
}

export type JobRating = 'good' | 'fair' | 'poor';

/** Live counterpart of JobDesignLog, pushed on `job://design` as a step runs. */
export interface DesignLogEvent {
  job_id: string;
  kind: 'begin' | 'delta' | 'reset' | 'end';
  step: string;
  label: string;
  text: string;
  status: string;
  timestamp: string;
}
