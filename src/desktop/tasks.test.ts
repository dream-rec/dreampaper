// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import type { JobRecord } from '../types';
import { desktopCopy } from './copy';
import { TaskStrip } from './shell';
import {
  activeTab,
  addTab,
  applyJob,
  closeTab,
  liveJobs,
  MAX_TABS,
  openBook,
  patchTab,
  selectTab,
  tabForJob,
  tabLabel
} from './tasks';

function job(id: string, status: JobRecord['status'] = 'running', title?: string): JobRecord {
  return { id, mode: 'paper_figure', status, created_at: '', updated_at: '', images: [], title };
}

describe('task book', () => {
  it('opens with one active tab and appends new ones up to the cap', () => {
    let book = openBook({ title: '' });
    expect(book.tabs).toHaveLength(1);
    expect(activeTab(book).seq).toBe(1);
    for (let i = 1; i < MAX_TABS; i += 1) book = addTab(book, { title: `t${i}` });
    expect(book.tabs).toHaveLength(MAX_TABS);
    expect(activeTab(book).form.title).toBe(`t${MAX_TABS - 1}`);
    const full = addTab(book, { title: 'overflow' });
    expect(full).toBe(book);
  });

  it('isolates form and job per tab', () => {
    let book = openBook({ title: 'a' });
    const first = book.active;
    book = addTab(book, { title: 'b' });
    book = patchTab(book, first, () => ({ job: job('j1') }));
    expect(tabForJob(book, 'j1')?.id).toBe(first);
    expect(activeTab(book).job).toBeNull();
    expect(activeTab(book).form.title).toBe('b');
    // A polled record lands on the tab that owns the job, active or not.
    book = applyJob(book, job('j1', 'succeeded'));
    expect(book.tabs.find((tab) => tab.id === first)?.job?.status).toBe('succeeded');
    expect(applyJob(book, job('unknown'))).toBe(book);
  });

  it('closing focuses the left neighbour and never leaves the book empty', () => {
    let book = openBook({ title: 'a' });
    const a = book.active;
    book = addTab(book, { title: 'b' });
    const b = book.active;
    book = addTab(book, { title: 'c' });
    const c = book.active;
    book = closeTab(book, c);
    expect(book.active).toBe(b);
    book = selectTab(book, a);
    book = closeTab(book, b);
    expect(book.active).toBe(a);
    expect(book.tabs).toHaveLength(1);
    // The last tab cannot be removed, and its number does not advance.
    book = patchTab(book, a, () => ({ job: job('j') }));
    const stuck = closeTab(book, a);
    expect(stuck).toBe(book);
    expect(activeTab(stuck).job?.id).toBe('j');
    expect(activeTab(stuck).form.title).toBe('a');
    expect(activeTab(stuck).seq).toBe(1);
  });

  it('reports only unsettled jobs as live and labels tabs by title', () => {
    let book = openBook({ title: '' });
    const a = book.active;
    book = addTab(book, { title: '第二个很长很长很长很长的标题标题标题' });
    book = patchTab(book, a, () => ({ job: job('j1', 'running') }));
    book = patchTab(book, book.active, () => ({ job: job('j2', 'failed') }));
    expect(liveJobs(book).map((item) => item.id)).toEqual(['j1']);
    const fallback = (n: number) => `任务 ${n}`;
    expect(tabLabel(book.tabs[0], (form) => form.title, fallback)).toBe('任务 1');
    expect(tabLabel(book.tabs[1], (form) => form.title, fallback)).toBe('第二个很长很长很长很长的标题…');
    book = patchTab(book, a, () => ({ job: job('j1', 'succeeded', 'Job title') }));
    expect(tabLabel(book.tabs[0], (form) => form.title, fallback)).toBe('Job title');
  });
});

describe('只剩一个任务页时', () => {
  let root: Root | null = null;

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    document.body.innerHTML = '';
  });

  function render(book: ReturnType<typeof openBook<{ title: string }>>, onClose: (id: string) => void) {
    const host = document.createElement('div');
    document.body.append(host);
    root = createRoot(host);
    act(() => {
      root!.render(
        createElement(TaskStrip<{ title: string }>, {
          book,
          d: desktopCopy.zh,
          formTitle: (form) => form.title,
          onSelect: () => {},
          onAdd: () => {},
          onClose
        })
      );
    });
    return host;
  }

  it('点删除会提示无法删除，任务页保持不动', () => {
    const book = openBook({ title: '' });
    let closed = false;
    const host = render(book, () => {
      closed = true;
    });
    const close = host.querySelector<HTMLButtonElement>('.task-tab-close');
    act(() => close?.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(closed).toBe(false);
    expect(document.body.textContent).toContain('唯一任务，无法删除');
    const ack = document.body.querySelector<HTMLButtonElement>('.task-only-btn');
    act(() => ack?.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(document.body.querySelector('.task-only')).toBeNull();
    expect(host.querySelector('.task-tab-label')?.textContent).toBe('任务 1');
  });
});
