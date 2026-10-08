import type { JobRecord } from '../types';

/**
 * One task tab on the figure or slide page: its own form, its own job.
 *
 * Tabs are session state — closing the app drops them, the jobs stay in
 * history. Isolation is by value: nothing here is shared between tabs.
 */
export type TaskTab<F> = {
  id: string;
  /** 1-based ordinal used for the default "任务 N" label; never reused. */
  seq: number;
  form: F;
  job: JobRecord | null;
};

export type TaskBook<F> = {
  tabs: TaskTab<F>[];
  active: string;
  nextSeq: number;
};

export const MAX_TABS = 8;

let idCounter = 0;
function nextId(): string {
  idCounter += 1;
  return `task-${Date.now().toString(36)}-${idCounter}`;
}

export function openBook<F>(form: F): TaskBook<F> {
  const tab: TaskTab<F> = { id: nextId(), seq: 1, form, job: null };
  return { tabs: [tab], active: tab.id, nextSeq: 2 };
}

export function activeTab<F>(book: TaskBook<F>): TaskTab<F> {
  return book.tabs.find((tab) => tab.id === book.active) ?? book.tabs[0];
}

/** Appends a tab and makes it active; returns the book unchanged when full. */
export function addTab<F>(book: TaskBook<F>, form: F, job: JobRecord | null = null): TaskBook<F> {
  if (book.tabs.length >= MAX_TABS) return book;
  const tab: TaskTab<F> = { id: nextId(), seq: book.nextSeq, form, job };
  return { tabs: [...book.tabs, tab], active: tab.id, nextSeq: book.nextSeq + 1 };
}

export function selectTab<F>(book: TaskBook<F>, id: string): TaskBook<F> {
  return book.tabs.some((tab) => tab.id === id) ? { ...book, active: id } : book;
}

/**
 * Removes a tab. The last tab stays: the page always has one task, and its
 * number does not advance. Focus moves to the neighbour on the left.
 */
export function closeTab<F>(book: TaskBook<F>, id: string): TaskBook<F> {
  const index = book.tabs.findIndex((tab) => tab.id === id);
  if (index === -1 || book.tabs.length === 1) return book;
  const tabs = book.tabs.filter((tab) => tab.id !== id);
  const active =
    book.active === id ? tabs[Math.max(0, index - 1)].id : book.active;
  return { ...book, tabs, active };
}

export function patchTab<F>(
  book: TaskBook<F>,
  id: string,
  patch: (tab: TaskTab<F>) => Partial<TaskTab<F>>
): TaskBook<F> {
  let changed = false;
  const tabs = book.tabs.map((tab) => {
    if (tab.id !== id) return tab;
    changed = true;
    return { ...tab, ...patch(tab) };
  });
  return changed ? { ...book, tabs } : book;
}

/** Applies a polled job record to whichever tab is showing that job. */
export function applyJob<F>(book: TaskBook<F>, job: JobRecord): TaskBook<F> {
  const tab = book.tabs.find((item) => item.job?.id === job.id);
  return tab ? patchTab(book, tab.id, () => ({ job })) : book;
}

export function tabForJob<F>(book: TaskBook<F>, jobId: string): TaskTab<F> | undefined {
  return book.tabs.find((tab) => tab.job?.id === jobId);
}

/** Jobs still running across every tab, for the shared poller. */
export function liveJobs<F>(book: TaskBook<F>): JobRecord[] {
  return book.tabs
    .map((tab) => tab.job)
    .filter((job): job is JobRecord => Boolean(job) && job!.status !== 'succeeded' && job!.status !== 'failed' && job!.status !== 'cancelled');
}

/** Short tab label: the job or form title when there is one, else "任务 N". */
export function tabLabel<F>(tab: TaskTab<F>, formTitle: (form: F) => string, fallback: (seq: number) => string): string {
  const title = (tab.job?.title ?? formTitle(tab.form)).trim().split('\n')[0] ?? '';
  if (!title) return fallback(tab.seq);
  return title.length > 14 ? `${title.slice(0, 14)}…` : title;
}
