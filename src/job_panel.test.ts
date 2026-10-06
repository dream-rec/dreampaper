// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { JobPanel, copy, useDesignLogs, useFinalPrompts, useJobPolling } from './app';
import type { DesignLogEvent, JobDesignLog, JobRecord } from './types';

/**
 * 面板级回归：订阅清理、轮询竞态与复制反馈都要按任务隔离。
 *
 * `listenDesignLog` 的承诺和 `getJob` 的响应都由测试放行，这样"切换任务之后
 * 旧订阅/旧请求才返回"的时序可以精确复现。
 */
const state = vi.hoisted(() => ({
  handlers: [] as Array<(event: DesignLogEvent) => void>,
  release: [] as Array<() => void>,
  unsubscribed: 0,
  polls: [] as Array<{ id: string; resolve: (job: JobRecord) => void }>,
  resumed: [] as string[]
}));

vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  // 停止/继续按钮只在桌面壳里出现，测试里当成桌面端。
  desktopAvailable: () => true,
  resumeJob: (id: string) => {
    state.resumed.push(id);
    return Promise.resolve({ ...job(id), status: 'running' } as JobRecord);
  },
  listenDesignLog: (handler: (event: DesignLogEvent) => void) =>
    new Promise<() => void>((resolve) => {
      state.handlers.push(handler);
      state.release.push(() =>
        resolve(() => {
          state.unsubscribed += 1;
        })
      );
    }),
  getJob: (id: string) =>
    new Promise<JobRecord>((resolve) => {
      state.polls.push({ id, resolve });
    })
}));

function job(id: string, logs: JobDesignLog[] = [], status: JobRecord['status'] = 'running'): JobRecord {
  return {
    id,
    mode: 'paper_figure',
    status,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    images: [],
    events: [],
    design_logs: logs
  };
}

function finalLog(content: string, page = 1): JobDesignLog {
  return {
    step: `final_prompt_${page}`,
    label: `最终制图提示词 ${page}`,
    status: 'succeeded',
    content,
    timestamp: '2026-01-01T00:00:00Z'
  };
}

function event(jobId: string, text: string, kind: DesignLogEvent['kind'] = 'end'): DesignLogEvent {
  return {
    job_id: jobId,
    kind,
    step: 'final_prompt_1',
    label: '最终制图提示词 1',
    text,
    status: 'succeeded',
    timestamp: '2026-01-01T00:00:00Z'
  };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  state.handlers.length = 0;
  state.release.length = 0;
  state.polls.length = 0;
  state.unsubscribed = 0;
  state.resumed.length = 0;
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
});

function copyLabels(): string[] {
  return [...container.querySelectorAll('.flow-copy')].map((button) => button.textContent?.trim() ?? '');
}

function stageLabels(): string[] {
  return [...container.querySelectorAll('.flow-stage')].map((stage) => stage.getAttribute('aria-label') ?? '');
}

async function clickCopy() {
  const button = container.querySelector<HTMLButtonElement>('.flow-copy');
  expect(button, '复制按钮').not.toBeNull();
  await act(async () => {
    button!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
  });
}

function useClipboard(writeText: () => Promise<void>) {
  Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true });
}

function PromptProbe({ record }: { record: JobRecord }) {
  const cards = useFinalPrompts(record);
  return createElement('span', null, cards.map((card) => card.text).join('|'));
}

function PollProbe({ record, onUpdate }: { record: JobRecord; onUpdate: (next: JobRecord) => void }) {
  useJobPolling(record, onUpdate, () => {});
  return createElement('span', null, record.id);
}

async function render(element: ReturnType<typeof createElement>) {
  await act(async () => {
    root.render(element);
  });
}

describe('JobPanel 复制反馈', () => {
  it('只在当前任务上显示已复制', async () => {
    const writeText = vi.fn(async () => undefined);
    useClipboard(writeText);
    await render(createElement(JobPanel, { job: job('job-1', [finalLog('prompt one')]), t: copy.zh }));
    expect(copyLabels()).toEqual([copy.zh.result.copy]);

    await clickCopy();
    expect(writeText).toHaveBeenCalledWith('prompt one');
    expect(copyLabels()).toEqual([copy.zh.result.copied]);

    // 同名 step 的另一个任务不能继承上一次的“已复制”。
    await render(createElement(JobPanel, { job: job('job-2', [finalLog('prompt two')]), t: copy.zh }));
    expect(copyLabels()).toEqual([copy.zh.result.copy]);
  });

  it('剪贴板不可用时把失败交给 onError', async () => {
    useClipboard(async () => {
      throw new Error('clipboard denied');
    });
    const onError = vi.fn();
    await render(createElement(JobPanel, { job: job('job-1', [finalLog('prompt one')]), t: copy.zh, onError }));
    await clickCopy();
    expect(onError).toHaveBeenCalledWith(copy.zh.result.copyFailed);
    expect(copyLabels()).toEqual([copy.zh.result.copy]);
  });
});

describe('JobPanel 生成过程', () => {
  it('把一次联网查询的查询内容与结果收进同一行', async () => {
    const record = job(
      'job-1',
      [
        {
          step: 'search_request_1',
          label: '检索请求 1 · Docker',
          status: 'succeeded',
          content: '{"model":"grok-build-0.1","tool_choice":"required"}',
          timestamp: '2026-01-01T00:00:00Z'
        },
        {
          step: 'search_response_1',
          label: '检索返回 1 · Docker',
          status: 'succeeded',
          content: '{"choices":[]}',
          timestamp: '2026-01-01T00:00:00Z'
        }
      ],
      'succeeded'
    );
    await render(createElement(JobPanel, { job: record, t: copy.zh }));

    const flow = container.querySelector('.flow-log');
    expect(flow, '生成过程区').not.toBeNull();
    expect(flow!.textContent).toContain('grok-build-0.1');
    expect(flow!.textContent).toContain('{"choices":[]}');
    // 查询与结果是同一行里的两块，用面向用户的说法，不出现技术词。
    const labels = [...container.querySelectorAll('.flow-block-label')].map((node) => node.textContent);
    expect(labels).toEqual(['查询内容', '查询结果']);
    expect(container.querySelectorAll('.flow-item.search')).toHaveLength(1);
    expect(flow!.textContent).not.toContain('请求体');
    expect(flow!.textContent).not.toContain('上游');
  });
});

describe('useFinalPrompts 订阅隔离', () => {
  it('清理后到达的旧任务事件不会顶掉当前事件缓存', async () => {
    await render(createElement(PromptProbe, { record: job('job-1') }));
    expect(state.handlers).toHaveLength(1);
    await act(async () => {
      state.release.splice(0).forEach((release) => release());
    });
    const stale = state.handlers[0];

    await render(createElement(PromptProbe, { record: job('job-2') }));
    expect(state.handlers).toHaveLength(2);
    expect(state.unsubscribed).toBe(1);
    await act(async () => {
      state.handlers[1](event('job-2', 'fresh'));
    });
    expect(container.textContent).toBe('fresh');

    // 旧订阅的清理已经跑过：它的事件既不能顶掉当前任务，也不能留在缓存里。
    await act(async () => {
      stale(event('job-1', 'stale'));
    });
    expect(container.textContent).toBe('fresh');

    // 切回 job-1：陈旧文本不能借道事件缓存回来。
    await render(createElement(PromptProbe, { record: job('job-1') }));
    expect(container.textContent).toBe('');
  });

  it('忽略增量事件，只认完整结束事件', async () => {
    await render(createElement(PromptProbe, { record: job('job-1') }));
    await act(async () => {
      state.handlers[0](event('job-1', 'half', 'delta'));
    });
    expect(container.textContent).toBe('');
  });
});

describe('useJobPolling 响应隔离', () => {
  it('切换任务后迟到的响应被丢弃', async () => {
    vi.useFakeTimers();
    const onUpdate = vi.fn();
    await render(createElement(PollProbe, { record: job('job-1'), onUpdate }));
    await act(async () => {
      vi.advanceTimersByTime(1800);
    });
    expect(state.polls.map((poll) => poll.id)).toEqual(['job-1']);

    await render(createElement(PollProbe, { record: job('job-2'), onUpdate }));
    await act(async () => {
      state.polls[0].resolve(job('job-1', [], 'succeeded'));
    });
    expect(onUpdate).not.toHaveBeenCalled();
    expect(container.textContent).toBe('job-2');
  });
});

function designLog(step: string, content = step): JobDesignLog {
  return { step, label: step, content, status: 'succeeded', timestamp: '2026-01-01T00:00:00Z' };
}

function DesignProbe({ record }: { record: JobRecord }) {
  return createElement('span', null, useDesignLogs(record).map((card) => card.text).join('|'));
}

describe('JobPanel 阶段与产物隔离', () => {
  it('进度下方独立排列 Search、Design，并把最终提示词放到独立 Implement', async () => {
    const record = job('simple', [
      designLog('ppt_visual_assets', 'grounded description and reference'),
      designLog('ppt_analyze', 'master analysis'),
      finalLog('final drawing prompt')
    ], 'succeeded');
    record.mode = 'ppt_slide';
    record.payload = { mode: 'ppt_slide', simple_mode: true, payload: {} };
    await render(createElement(JobPanel, { job: record, t: copy.zh }));
    const progress = container.querySelector('.progress-card')!;
    const process = container.querySelector('.flow-log')!;
    const implement = container.querySelector('.implement-results')!;
    expect(progress.contains(process)).toBe(false);
    expect(progress.parentElement).toBe(process.parentElement);
    expect(process.nextElementSibling).toBe(implement.closest('.flow-stage-entry'));
    expect(progress.nextElementSibling).toBe(process);
    expect(process.nextElementSibling!.querySelector('.implement-results')).toBe(implement);
    expect([...process.querySelectorAll('.flow-stage')].map((stage) => stage.getAttribute('aria-label')))
      .toEqual(['Search', 'Design']);
    expect(process.querySelector('.flow-stage-search')!.textContent).toContain('grounded description and reference');
    expect(process.querySelector('.flow-stage-design')!.textContent).toContain('master analysis');
    expect(process.textContent).not.toContain('final drawing prompt');
    expect(process.querySelector('.flow-copy')).toBeNull();
    expect(implement.textContent).toContain('final drawing prompt');
    expect(implement.querySelector('.flow-toggle')!.getAttribute('aria-expanded')).toBe('true');
    expect(implement.querySelector('.flow-copy')).not.toBeNull();
    expect(implement.querySelector('.result-grid')).toBeNull();
  });

  it('普通模式的 Implement 只展示结果图，不能被日志中的提示词替代', async () => {
    const record = job('ordinary', [designLog('paper_design'), finalLog('not the output')], 'succeeded');
    record.payload = { mode: 'paper_figure', simple_mode: false, payload: {} };
    record.images = [{ name: 'result.png', url: 'https://example.com/result.png', asset_id: 'asset-1' }];
    await render(createElement(JobPanel, { job: record, t: copy.en }));
    const implement = container.querySelector('.implement-results')!;
    expect(implement.querySelector('img')!.getAttribute('alt')).toBe('result.png');
    expect(implement.querySelector('.final-prompt-list')).toBeNull();
    expect(container.querySelector('.flow-log img')).toBeNull();
    expect(implement.querySelector('.result-download')).not.toBeNull();
  });

  it('阶段卡片跟着任务进度出现：先 Search，再 Design，最后 Implement', async () => {
    const search = designLog('search_request_1', '{"model":"grok"}');
    await render(createElement(JobPanel, { job: job('live', [search]), t: copy.zh }));
    expect(stageLabels()).toEqual(['Search']);

    await render(createElement(JobPanel, { job: job('live', [search, designLog('paper_structure')]), t: copy.zh }));
    expect(stageLabels()).toEqual(['Search', 'Design']);
    expect(container.querySelector('.implement-results')).toBeNull();

    // 进入制图阶段就出现 Implement，不必等图片保存回来。
    const drawing = job('live', [search, designLog('paper_structure')]);
    drawing.stage = 'paper_implement';
    await render(createElement(JobPanel, { job: drawing, t: copy.zh }));
    expect(stageLabels()).toEqual(['Search', 'Design', 'Implement']);
    expect(container.querySelector('.implement-results .flow-empty')).not.toBeNull();
  });

  it('等待中的任务不显示任何阶段卡片，上一个任务的结果也不会留下', async () => {
    await render(createElement(JobPanel, { job: job('old', [finalLog('old prompt')], 'succeeded'), t: copy.zh }));
    await render(createElement(JobPanel, { job: job('new', [], 'queued'), t: copy.zh }));
    expect(container.textContent).not.toContain('old prompt');
    expect(stageLabels()).toEqual([]);
    expect(container.querySelectorAll('.flow-empty')).toHaveLength(0);
  });

  it('轮询新增已完成步骤时更新当前阶段，已有折叠行不会被重新挂载', async () => {
    const first = designLog('ppt_analyze', 'analysis');
    await render(createElement(JobPanel, { job: job('poll', [first]), t: copy.zh }));
    const original = container.querySelector('.flow-stage-design .flow-entry');
    const toggle = original!.querySelector<HTMLButtonElement>('.flow-toggle')!;
    await act(async () => toggle.click());
    await render(createElement(JobPanel, {
      job: job('poll', [first, designLog('ppt_outline', 'new outline')]), t: copy.zh
    }));
    expect(container.querySelector('.flow-stage-design .flow-entry')).toBe(original);
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    const entries = container.querySelectorAll('.flow-stage-design .flow-entry');
    expect(entries).toHaveLength(2);
    // Completed/non-streaming arrivals share the same insertion-animation wrapper.
    expect(entries[1].querySelector('.flow-item.succeeded')).not.toBeNull();
    expect(entries[1].textContent).toContain('new outline');
  });

  it('流式步骤只在初次出现时自动展开，用户收起后不会被新文本强行展开', async () => {
    await render(createElement(JobPanel, { job: job('live'), t: copy.zh }));
    const push = async (kind: DesignLogEvent['kind'], text = '') => act(async () => {
      state.handlers.forEach((handler) => handler({
        ...event('live', text, kind), step: 'paper_design', label: 'figure plan', status: 'running'
      }));
    });
    await push('begin');
    const toggle = container.querySelector<HTMLButtonElement>('.flow-stage-design .flow-toggle')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    await act(async () => toggle.click());
    await push('delta', 'first chunk');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    const body = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(body.getAttribute('aria-hidden')).toBe('true');
    expect(body.hasAttribute('inert')).toBe(true);
    expect(body.textContent).toContain('first chunk');
  });

  it('还在接收又还没有内容时不铺空块，结束之后才给出占位', async () => {
    await render(createElement(JobPanel, { job: job('live'), t: copy.zh }));
    const push = async (kind: DesignLogEvent['kind'], status: string) => act(async () => {
      state.handlers.forEach((handler) => handler({
        ...event('live', '', kind), step: 'paper_design', label: 'figure plan', status
      }));
    });
    await push('begin', 'running');
    expect(container.querySelector('.flow-stage-design .flow-text')).toBeNull();
    await push('end', 'succeeded');
    expect(container.querySelector('.flow-stage-design .flow-text')?.textContent).toBe(copy.zh.result.stepEmpty);
  });

  it('已结束任务不会继续显示流式接收状态或活动高亮', async () => {
    const record = job('stopped');
    record.stage = 'paper_design';
    await render(createElement(JobPanel, { job: record, t: copy.zh }));
    await act(async () => state.handlers.forEach((handler) => handler({
      ...event('stopped', '', 'begin'), step: 'paper_design', status: 'running'
    })));
    await render(createElement(JobPanel, { job: { ...record, status: 'cancelled' }, t: copy.zh }));
    expect(container.querySelector('.flow-badge')).toBeNull();
    expect(container.querySelector('.flow-item.active')).toBeNull();
  });
});

describe('useDesignLogs 流式与存储合并', () => {
  it('较旧的持久化结果不能遮住同一步重试，但后续最终轮询结果仍然优先', async () => {
    const prior = designLog('paper_design', 'invalid first attempt');
    await render(createElement(DesignProbe, { record: job('retry', [prior]) }));
    await act(async () => state.handlers[0]({
      ...event('retry', 'second attempt', 'delta'), step: 'paper_design', timestamp: '2026-01-01T00:00:01Z'
    }));
    await render(createElement(DesignProbe, { record: job('retry', [prior]) }));
    expect(container.textContent).toBe('second attempt');
    await render(createElement(DesignProbe, { record: job('retry', [{
      ...prior, content: 'final successful plan', timestamp: '2026-01-01T00:00:02Z'
    }]) }));
    expect(container.textContent).toBe('final successful plan');
  });

  it('重试清空旧片段，最终输出覆盖增量，其他阶段事件不混入 Design', async () => {
    await render(createElement(DesignProbe, { record: job('stream') }));
    const push = async (kind: DesignLogEvent['kind'], text: string, step = 'paper_design') => act(async () => {
      state.handlers[0]({ ...event('stream', text, kind), step });
    });
    await push('begin', '');
    await push('delta', 'first');
    await push('delta', ' draft');
    expect(container.textContent).toBe('first draft');
    await push('reset', '');
    expect(container.textContent).toBe('');
    await push('delta', 'retry');
    await push('end', 'authoritative output');
    await push('end', 'search trace', 'search_response_1');
    await push('end', 'final product', 'final_prompt_1');
    expect(container.textContent).toBe('authoritative output');
  });

  it('旧任务的迟到事件不能覆盖新任务', async () => {
    await render(createElement(DesignProbe, { record: job('old') }));
    const stale = state.handlers[0];
    await render(createElement(DesignProbe, { record: job('new', [designLog('paper_design', 'new design')]) }));
    await act(async () => stale({ ...event('old', 'stale design'), step: 'paper_design' }));
    expect(container.textContent).toBe('new design');
    await render(createElement(DesignProbe, { record: job('old') }));
    expect(container.textContent).toBe('');
  });

  it('轮询不丢失正在流入的文本，并以最终存储输出为准', async () => {
    await render(createElement(DesignProbe, { record: job('live') }));
    await act(async () => state.handlers[0]({ ...event('live', 'partial', 'delta'), step: 'paper_design' }));
    await render(createElement(DesignProbe, { record: job('live', [designLog('paper_structure', 'structure')]) }));
    expect(container.textContent).toBe('structure|partial');
    await render(createElement(DesignProbe, { record: job('live', [
      designLog('paper_structure', 'structure'), designLog('paper_design', 'complete')
    ]) }));
    expect(container.textContent).toBe('structure|complete');
  });
});

describe('JobPanel 继续 / 重试', () => {
  function resumeButton() {
    return container.querySelector<HTMLButtonElement>('.resume-job-button');
  }

  async function clickResume() {
    const button = resumeButton();
    expect(button, '继续按钮').not.toBeNull();
    await act(async () => {
      button!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
  }

  it('失败的任务给出重试，点击后回到运行中', async () => {
    const resumed: JobRecord[] = [];
    await render(
      createElement(JobPanel, {
        job: job('job-failed', [], 'failed'),
        t: copy.zh,
        onResumed: (next) => resumed.push(next)
      })
    );
    expect(resumeButton()?.textContent).toBe(copy.zh.result.retry);
    await clickResume();
    expect(state.resumed).toEqual(['job-failed']);
    expect(resumed.map((item) => item.status)).toEqual(['running']);
  });

  it('停止过的任务给出继续任务', async () => {
    await render(
      createElement(JobPanel, {
        job: job('job-stopped', [], 'cancelled'),
        t: copy.zh,
        onResumed: () => {}
      })
    );
    expect(resumeButton()?.textContent).toBe(copy.zh.result.resume);
  });

  it('成功完成的任务既不能继续也不能停止', async () => {
    await render(
      createElement(JobPanel, {
        job: job('job-done', [], 'succeeded'),
        t: copy.zh,
        onCancelled: () => {},
        onResumed: () => {}
      })
    );
    expect(resumeButton()).toBeNull();
    expect(container.querySelector('.stop-job-button')).toBeNull();
  });

  it('还在跑的任务只显示停止', async () => {
    await render(
      createElement(JobPanel, {
        job: job('job-live', [], 'running'),
        t: copy.zh,
        onCancelled: () => {},
        onResumed: () => {}
      })
    );
    expect(resumeButton()).toBeNull();
    expect(container.querySelector('.stop-job-button')).not.toBeNull();
  });
});
