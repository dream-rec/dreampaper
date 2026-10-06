// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { desktopCopy } from './copy';
import { HistoryCard, HistoryPage } from './shell';
import type { JobRecord } from '../types';

const state = vi.hoisted(() => ({ rows: [] as JobRecord[] }));

vi.mock('../api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api')>()),
  listJobs: () => Promise.resolve(state.rows),
  desktopAvailable: () => true
}));

/**
 * 历史卡片的预览来源：普通模式看成品图，简单模式看所选母版，
 * 母版被删掉时退回空图标而不是留一张碎图。
 */
const d = desktopCopy.zh;

function job(overrides: Partial<JobRecord> = {}): JobRecord {
  return {
    id: 'job-1',
    mode: 'ppt_slide',
    status: 'succeeded',
    created_at: '2026-10-06T12:00:00Z',
    updated_at: '2026-10-06T12:00:00Z',
    images: [],
    ...overrides
  };
}

let root: Root | null = null;
let host: HTMLDivElement | null = null;

function render(record: JobRecord) {
  host = document.createElement('div');
  document.body.append(host);
  root = createRoot(host);
  act(() => {
    root!.render(
      createElement(HistoryCard, {
        job: record,
        d,
        filteredOut: false,
        confirming: false,
        onOpen: () => {},
        onPreview: () => {},
        onDelete: () => {},
        onRerun: () => {},
        onOpenWorkbench: () => {}
      })
    );
  });
  return host;
}

beforeEach(() => {
  document.body.innerHTML = '';
});

afterEach(() => {
  act(() => root?.unmount());
  root = null;
  host = null;
});

describe('历史卡片预览', () => {
  it('简单模式显示所选母版，并标明这次没有成品图', () => {
    const node = render(
      job({ simple: true, thumbnail: 'http://dp-template.localhost/tpl-1?w=800' })
    );
    const image = node.querySelector('.hc-image img');
    expect(image?.getAttribute('src')).toBe('http://dp-template.localhost/tpl-1?w=800');
    expect(node.querySelector('.hc-simple')?.textContent).toBe(d.history.simple);
    expect(node.querySelector('.hc-image')?.getAttribute('title')).toContain(d.history.reference);
    expect(node.querySelector('.hc-image-empty')).toBeNull();
  });

  it('普通模式用成品图，不带简单模式标签', () => {
    const node = render(job({ thumbnail: 'http://dp-asset.localhost/done?w=800' }));
    expect(node.querySelector('.hc-image img')?.getAttribute('src')).toBe(
      'http://dp-asset.localhost/done?w=800'
    );
    expect(node.querySelector('.hc-simple')).toBeNull();
    expect(node.querySelector('.hc-image')?.getAttribute('title')).toBe(d.history.zoom);
  });

  it('没有预览就显示图标', () => {
    const node = render(job({ simple: true }));
    expect(node.querySelector('.hc-image img')).toBeNull();
    expect(node.querySelector('.hc-image-empty')).not.toBeNull();
  });

  it('预览图加载失败（母版被删）时退回图标', () => {
    const node = render(job({ simple: true, thumbnail: 'http://dp-template.localhost/gone?w=800' }));
    expect(node.querySelector('.hc-image img')).not.toBeNull();
    act(() => {
      node.querySelector('.hc-image img')!.dispatchEvent(new Event('error'));
    });
    expect(node.querySelector('.hc-image img')).toBeNull();
    expect(node.querySelector('.hc-image-empty')).not.toBeNull();
  });
});

/**
 * 筛选条：简单/普通模式与其它条件一样，只切换可见性、不重建卡片。
 */
describe('历史筛选', () => {
  async function renderPage(rows: JobRecord[]) {
    state.rows = rows;
    host = document.createElement('div');
    document.body.append(host);
    root = createRoot(host);
    act(() => {
      root!.render(
        createElement(HistoryPage, {
          d,
          onOpen: () => {},
          onDelete: () => {},
          onRerun: () => {},
          onOpenWorkbench: () => {}
        })
      );
    });
    // listJobs 是 Promise，等它落地后卡片才会渲染。
    await act(async () => {
      await Promise.resolve();
    });
    return host;
  }

  function visibleIds(node: HTMLElement) {
    return Array.from(node.querySelectorAll('.history-card'))
      .filter((card) => !card.classList.contains('is-hidden'))
      .map((card) => card.querySelector('.hc-title')?.textContent);
  }

  it('按运行模式筛选只保留对应的一类', async () => {
    const node = await renderPage([
      job({ id: 'a', title: '简单任务', simple: true }),
      job({ id: 'b', title: '普通任务' })
    ]);
    expect(visibleIds(node)).toEqual(['简单任务', '普通任务']);

    const seg = node.querySelector('[aria-label="按运行模式筛选"]')!;
    const pick = (label: string) => {
      const button = Array.from(seg.querySelectorAll('button')).find(
        (item) => item.textContent === label
      )!;
      act(() => button.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    };

    pick(d.history.simple);
    expect(visibleIds(node)).toEqual(['简单任务']);

    pick(d.history.normal);
    expect(visibleIds(node)).toEqual(['普通任务']);

    pick(d.history.all);
    expect(visibleIds(node)).toEqual(['简单任务', '普通任务']);
  });
});
