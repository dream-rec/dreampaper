import { describe, expect, it } from 'vitest';
import { mergeFinalPrompts } from './app';
import type { JobDesignLog } from './types';

function log(step: string, content: string, status = 'succeeded'): JobDesignLog {
  return { step, label: `label ${step}`, status, content, timestamp: '2026-01-01T00:00:00Z' };
}

const empty = { jobId: null, cards: [] };

describe('mergeFinalPrompts', () => {
  it('reads a reopened job from its stored rows', () => {
    const prompts = mergeFinalPrompts('job-1', [log('final_prompt_1', 'draw a pipeline')], { ...empty, jobId: 'job-1' });
    expect(prompts.map((card) => card.text)).toEqual(['draw a pipeline']);
    expect(prompts[0].status).toBe('succeeded');
  });

  it('keeps the stored row when the event was missed or arrives first', () => {
    const recorded = [log('final_prompt_1', 'stored')];
    const streamed = { jobId: 'job-1', cards: [{ step: 'final_prompt_1', label: 'Live', text: 'live', status: 'running' as const }] };
    expect(mergeFinalPrompts('job-1', recorded, streamed)[0].text).toBe('stored');
    expect(mergeFinalPrompts('job-1', [], streamed)[0].text).toBe('live');
  });

  it('shows a streamed prompt for a job with nothing persisted yet', () => {
    const streamed = {
      jobId: 'job-1',
      cards: [{ step: 'final_prompt_1', label: '最终制图提示词 1', text: 'live', status: 'succeeded' as const }]
    };
    expect(mergeFinalPrompts('job-1', undefined, streamed)).toHaveLength(1);
  });

  it('never shows another job\'s prompt', () => {
    const streamed = {
      jobId: 'job-old',
      cards: [{ step: 'final_prompt_1', label: '', text: 'stale', status: 'succeeded' as const }]
    };
    expect(mergeFinalPrompts('job-new', [], streamed)).toEqual([]);
  });

  it('sorts slides by page number and ignores the process log', () => {
    const recorded = [
      log('final_prompt_10', 'page ten'),
      log('ppt_page_plan_1', 'process log'),
      log('final_prompt_2', 'page two'),
      log('final_prompt_1', 'page one')
    ];
    expect(mergeFinalPrompts('job-1', recorded, empty).map((card) => card.text)).toEqual([
      'page one',
      'page two',
      'page ten'
    ]);
  });

  it('carries a failed row through as failed', () => {
    expect(mergeFinalPrompts('job-1', [log('final_prompt_1', 'broken', 'failed')], empty)[0].status).toBe('failed');
  });
});
