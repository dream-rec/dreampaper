import { memo, useCallback, useEffect, useMemo, useRef, useState, type JSX } from 'react';
import packageMetadata from '../../package.json';
import {
  checkUpdate,
  createJob,
  deleteJob,
  desktopAvailable,
  getConfig,
  getJob,
  listJobs,
  listTemplates,
  openExternal,
  saveConfig
} from '../api';
import { copy, emptyConfig, isJobSettled, Settings, SettingsSection, SimpleModeSwitch, type Lang } from '../app';
import type { AppConfig, JobRecord } from '../types';
import { AboutPage, IconGitHub, type UpdateState } from './about';
import { desktopCopy, type DesktopCopy } from './copy';
import {
  defaultFigureForm,
  defaultSlideForm,
  FigureForm,
  SettingsPane,
  SlideForm,
  type FigureFormState,
  type SlideFormState
} from './forms';
import { TemplateLibrary } from './templates';
import { splitRules } from './style_picker';
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
  tabLabel,
  type TaskBook
} from './tasks';
import { WorkbenchPage, WorkbenchSettings, type LeaveGuard, type WorkbenchRequest, type WorkbenchSummary } from '../workbench';
import {
  applyTheme,
  autoUpdateEnabled,
  initialTheme,
  saveAutoUpdate,
  saveSimpleMode,
  saveTheme,
  simpleModePreference,
  themePreference,
  type Theme
} from './prefs';

type Page = 'paper' | 'ppt' | 'templates' | 'history' | 'workbench' | 'settings' | 'about';
type Copy = (typeof copy)[Lang];

// WebKitGTK 2.50 的 View Transition 会在 Linux 桌面端 SIGSEGV（tauri-apps/tauri#14721）。
// 这里也不再给滚动容器播位移动画：WebKit 不会把它交给合成器，切页的 240ms 里整页都在主线程重画。
const supportsViewTransitions =
  typeof document !== 'undefined' &&
  'startViewTransition' in document &&
  !(desktopAvailable() && /Linux/.test(navigator.userAgent));
let startupUpdateStarted = false;

function paneClass(id: Page, current: Page, viewTransition: boolean): string {
  const active = id === current;
  return [
    'desktop-pane',
    active ? 'is-active' : '',
    active && id === 'history' ? 'clips' : '',
    active && viewTransition ? 'has-view-transition' : ''
  ]
    .filter(Boolean)
    .join(' ');
}

export function DesktopApp() {
  const [page, setPage] = useState<Page>('paper');
  // Pages stay mounted after the first visit. Unmounting rebuilt the form,
  // the history grid, or the workbench canvas on every click.
  const [visited, setVisited] = useState<ReadonlySet<Page>>(() => new Set(['paper']));
  const [lang, setLang] = useState<Lang>('zh');
  const [theme, setTheme] = useState<Theme>(initialTheme);
  const [followsSystem, setFollowsSystem] = useState(() => themePreference() === null);
  const [autoUpdate, setAutoUpdate] = useState(autoUpdateEnabled);
  const [simpleMode, setSimpleMode] = useState(simpleModePreference);
  const [update, setUpdate] = useState<UpdateState>({ status: 'idle', info: null, error: null });
  const [config, setConfig] = useState<AppConfig>(emptyConfig);
  const [toast, setToast] = useState<{ id: number; text: string; tone: 'info' | 'error' } | null>(null);
  // Each page keeps a book of task tabs; a tab owns its form and its job, so
  // two figures drafted side by side never see each other's inputs or output.
  const [figureBook, setFigureBook] = useState<TaskBook<FigureFormState>>(() => openBook(defaultFigureForm));
  const [slideBook, setSlideBook] = useState<TaskBook<SlideFormState>>(() => openBook(defaultSlideForm));
  const figureTab = activeTab(figureBook);
  const slideTab = activeTab(slideBook);
  const [workbenchRequest, setWorkbenchRequest] = useState<WorkbenchRequest | null>(null);
  const [workbenchOpen, setWorkbenchOpen] = useState(false);
  const [workbenchSummary, setWorkbenchSummary] = useState<WorkbenchSummary>({ summary: '' });
  const workbenchLeaveGuard = useRef<LeaveGuard | null>(null);
  async function deleteSettled(job: JobRecord, refresh?: () => void) {
    try {
      await deleteJob(job.id);
      refresh?.();
    } catch (error) {
      showMessage(
        error instanceof Error ? error.message : d.recent.deleteFailed,
        'error'
      );
    }
  }

  async function rerunJob(job: JobRecord) {
    try {
      // List rows are kept light; fetch the full record for the stored payload.
      const full = await getJob(job.id);
      if (!full.payload) {
        showMessage(d.recent.rerunUnavailable, 'error');
        return;
      }
      // The rerun copies the stored envelope but takes today's switch: the job
      // about to run is a new job, and the mode is what the user sees selected.
      const created = await createJob({ ...full.payload, simple_mode: simpleMode });
      // The rerun reuses the stored payload, so the tab has to show the same
      // inputs it was built from; otherwise the panel contradicts the job. It
      // opens in a fresh tab so whatever the user was drafting stays put.
      const inputs = full.payload.payload;
      if (created.mode === 'ppt_slide') {
        openSlideTab(created, inputs);
        transitionToPage('ppt');
      } else {
        openFigureTab(created, inputs);
        transitionToPage('paper');
      }
    } catch (error) {
      showMessage(
        error instanceof Error ? error.message : d.recent.rerunFailed,
        'error'
      );
    }
  }

  const t = copy[lang];
  const d = desktopCopy[lang];

  const showPage = (nextPage: Page) => {
    setPage(nextPage);
    setVisited((current) => (current.has(nextPage) ? current : new Set(current).add(nextPage)));
  };

  const transitionToPage = async (nextPage: Page): Promise<boolean> => {
    if (nextPage === page) return true;
    if (page === 'workbench' && workbenchLeaveGuard.current && !(await workbenchLeaveGuard.current())) return false;
    if (supportsViewTransitions && !window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
      (document as any).startViewTransition(() => {
        showPage(nextPage);
      });
    } else {
      showPage(nextPage);
    }
    return true;
  };

  const openWorkbench = async (assetId: string) => {
    if (!(await transitionToPage('workbench'))) return;
    setWorkbenchRequest({ assetId, token: Date.now() });
  };

  const registerWorkbenchLeaveGuard = useCallback((guard: LeaveGuard | null) => {
    workbenchLeaveGuard.current = guard;
  }, []);

  const handleWorkbenchRequest = useCallback(() => setWorkbenchRequest(null), []);

  const showMessage = useMemo(
    () => (text: string, tone: 'info' | 'error' = 'info') => setToast({ id: Date.now(), text, tone }),
    []
  );

  useEffect(() => {
    if (!desktopAvailable()) return;
    let disposed = false;
    let stop: (() => void) | undefined;
    let closing = false;
    void import('@tauri-apps/api/window').then(async ({ getCurrentWindow }) => {
      const unsubscribe = await getCurrentWindow().onCloseRequested(async (event) => {
        if (closing) { event.preventDefault(); return; }
        closing = true;
        try {
          if (workbenchLeaveGuard.current && !(await workbenchLeaveGuard.current())) event.preventDefault();
        } catch (error) {
          event.preventDefault();
          showMessage(error instanceof Error ? error.message : '关闭前保存失败', 'error');
        } finally {
          closing = false;
        }
      });
      if (disposed) unsubscribe();
      else stop = unsubscribe;
    }).catch((error) => showMessage(String(error), 'error'));
    return () => { disposed = true; stop?.(); };
  }, [showMessage]);

  const runUpdateCheck = useCallback(async () => {
    setUpdate((current) => ({ status: 'checking', info: current.info, error: null }));
    try {
      const info = await checkUpdate();
      setUpdate({ status: info.update_available ? 'available' : 'current', info, error: null });
    } catch (error) {
      setUpdate((current) => ({
        status: 'failed',
        info: current.info,
        error: error instanceof Error ? error.message : null
      }));
    }
  }, []);

  useEffect(() => {
    applyTheme(theme);
    if (desktopAvailable()) {
      import('@tauri-apps/api/window')
        .then(({ getCurrentWindow }) => getCurrentWindow().setTheme(theme))
        .catch(() => {});
    }
  }, [theme]);

  useEffect(() => {
    if (!followsSystem) return;
    const query = window.matchMedia('(prefers-color-scheme: dark)');
    const sync = (event: MediaQueryListEvent) => setTheme(event.matches ? 'dark' : 'light');
    query.addEventListener('change', sync);
    return () => query.removeEventListener('change', sync);
  }, [followsSystem]);

  useEffect(() => {
    getConfig()
      .then((saved) => {
        setConfig(saved);
        if (autoUpdate && !startupUpdateStarted) {
          startupUpdateStarted = true;
          void runUpdateCheck();
        }
      })
      .catch((error) => showMessage(error.message, 'error'));
  }, [autoUpdate, runUpdateCheck, showMessage]);

  useBookPolling(figureBook, setFigureBook, showMessage);
  useBookPolling(slideBook, setSlideBook, showMessage);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3200);
    return () => window.clearTimeout(timer);
  }, [toast]);



  function toggleTheme() {
    const next = theme === 'dark' ? 'light' : 'dark';
    setFollowsSystem(false);
    saveTheme(next);
    setTheme(next);
  }

  function changeAutoUpdate(enabled: boolean) {
    saveAutoUpdate(enabled);
    setAutoUpdate(enabled);
  }

  function changeSimpleMode(enabled: boolean) {
    saveSimpleMode(enabled);
    setSimpleMode(enabled);
  }

  async function openUrl(url: string) {
    try {
      await openExternal(url);
    } catch (error) {
      showMessage(error instanceof Error ? error.message : d.about.openFailed, 'error');
    }
  }

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

  async function openJob(summary: JobRecord) {
    try {
      const job = await getJob(summary.id);
      if (job.mode === 'ppt_slide') {
        openSlideTab(job, job.payload?.payload);
        transitionToPage('ppt');
      } else {
        openFigureTab(job, job.payload?.payload);
        transitionToPage('paper');
      }
    } catch (error) {
      showMessage(error instanceof Error ? error.message : 'Load failed', 'error');
    }
  }

  // A job already shown in some tab is focused there rather than opened twice;
  // otherwise it gets a tab of its own with the original inputs restored so it
  // can be tweaked and resubmitted. When the book is full the active tab is
  // reused, which is the old single-slot behaviour.
  function openFigureTab(job: JobRecord, payload: unknown) {
    setFigureBook((book) => {
      const existing = tabForJob(book, job.id);
      if (existing) return selectTab(patchTab(book, existing.id, () => ({ job })), existing.id);
      const form = figureFormFrom(payload, defaultFigureForm, d.style);
      const next = addTab(book, form, job);
      if (next !== book) return next;
      showMessage(d.tasks.full(MAX_TABS), 'error');
      return patchTab(book, book.active, () => ({ job, form }));
    });
    pruneFigureTemplates(payload);
  }

  function openSlideTab(job: JobRecord, payload: unknown) {
    setSlideBook((book) => {
      const existing = tabForJob(book, job.id);
      if (existing) return selectTab(patchTab(book, existing.id, () => ({ job })), existing.id);
      const form = slideFormFrom(payload, defaultSlideForm, d.style);
      const next = addTab(book, form, job);
      if (next !== book) return next;
      showMessage(d.tasks.full(MAX_TABS), 'error');
      return patchTab(book, book.active, () => ({ job, form }));
    });
  }

  // Drop template ids that no longer exist so the picker and the ready flag
  // stay truthful. Runs after the tab exists, on whichever tab holds the job.
  function pruneFigureTemplates(payload: unknown) {
    const ids = figureFormFrom(payload, defaultFigureForm, d.style).selected;
    if (ids.length === 0) return;
    listTemplates('all', '')
      .then((templates) => {
        const available = new Set(templates.map((item) => item.id));
        const kept = ids.filter((id) => available.has(id));
        if (kept.length === ids.length) return;
        setFigureBook((book) =>
          patchTab(book, book.active, (tab) => ({ form: { ...tab.form, selected: kept } }))
        );
      })
      .catch(() => {});
  }

  const navItems: Array<{ key: Page; label: string; icon: JSX.Element }> = [
    { key: 'paper', label: d.nav.paper, icon: <IconFigure /> },
    { key: 'ppt', label: d.nav.ppt, icon: <IconSlide /> },
    { key: 'templates', label: d.nav.templates, icon: <IconTemplates /> },
    { key: 'history', label: d.nav.history, icon: <IconHistory /> },
    { key: 'workbench', label: d.nav.workbench, icon: <IconWorkbench /> }
  ];

  const head = pageHead[page];

  return (
    <div className="desktop-shell">
      <aside className="desktop-rail">
        <div className="desktop-rail-brand">
          <img src="/favor.png" alt="DreamPaper" />
        </div>

        <nav className="desktop-rail-nav" aria-label="Primary">
          {navItems.map((item) => (
            <button
              key={item.key}
              type="button"
              className={`rail-btn${page === item.key ? ' active' : ''}`}
              data-tip={item.label}
              aria-label={item.label}
              aria-current={page === item.key ? 'page' : undefined}
              onClick={() => transitionToPage(item.key)}
            >
              {item.icon}
            </button>
          ))}

        </nav>

        <span className="rail-spacer" />

        <button
          type="button"
          className="rail-btn"
          data-tip={theme === 'dark' ? d.theme.light : d.theme.dark}
          aria-label={theme === 'dark' ? d.theme.light : d.theme.dark}
          onClick={toggleTheme}
        >
          <IconTheme theme={theme} />
        </button>

        <button
          type="button"
          className={`rail-btn${page === 'settings' ? ' active' : ''}`}
          data-tip={d.nav.settings}
          aria-label={d.nav.settings}
          aria-current={page === 'settings' ? 'page' : undefined}
          onClick={() => transitionToPage('settings')}
        >
          <IconGear />
        </button>

        <button
          type="button"
          className="rail-btn"
          data-tip={lang === 'zh' ? 'English' : '中文'}
          aria-label={lang === 'zh' ? 'Switch to English' : '切换到中文'}
          onClick={() => setLang(lang === 'zh' ? 'en' : 'zh')}
        >
          <span className="rail-lang">{lang === 'zh' ? 'EN' : '中'}</span>
        </button>

        <button
          type="button"
          className={`rail-btn${page === 'about' ? ' active' : ''}`}
          data-tip={d.nav.about}
          aria-label={d.nav.about}
          aria-current={page === 'about' ? 'page' : undefined}
          onClick={() => transitionToPage('about')}
        >
          <IconGitHub />
        </button>
      </aside>

      <main className="desktop-main">
        <div className="desktop-drag-strip" data-tauri-drag-region />

        <div className="desktop-head" data-tauri-drag-region="deep">
          <div className="desktop-head-text">
            <h1>{head.title(t, d)}</h1>
            <p>{head.intro(t, d)}</p>
          </div>
          <SimpleModeSwitch
            on={simpleMode}
            label={d.simple.label}
            hint={d.simple.hint}
            onChange={changeSimpleMode}
          />
        </div>

        <div className="desktop-page">
          {visited.has('paper') && (
            <div className={paneClass('paper', page, supportsViewTransitions)}>
              <div className="task-page">
                <TaskStrip
                  book={figureBook}
                  d={d}
                  formTitle={(form) => form.title}
                  onSelect={(id) => setFigureBook((book) => selectTab(book, id))}
                  onAdd={() => setFigureBook((book) => addTab(book, defaultFigureForm))}
                  onClose={(id) => setFigureBook((book) => closeTab(book, id, defaultFigureForm))}
                />
                <FigureForm
                  key={figureTab.id}
                  simpleMode={simpleMode}
                  state={figureTab.form}
                  onState={(next) => setFigureBook((book) => patchTab(book, figureTab.id, (tab) => ({ form: next(tab.form) })))}
                  job={figureTab.job}
                  onJob={(job) => setFigureBook((book) => patchTab(book, figureTab.id, () => ({ job })))}
                  onMessage={showMessage}
                  onGoTemplates={() => void transitionToPage('templates')}
                  onOpenWorkbench={(assetId) => void openWorkbench(assetId)}
                  t={t}
                  d={d}
                />
              </div>
            </div>
          )}
          {visited.has('ppt') && (
            <div className={paneClass('ppt', page, supportsViewTransitions)}>
              <div className="task-page">
                <TaskStrip
                  book={slideBook}
                  d={d}
                  formTitle={(form) => form.material}
                  onSelect={(id) => setSlideBook((book) => selectTab(book, id))}
                  onAdd={() => setSlideBook((book) => addTab(book, defaultSlideForm))}
                  onClose={(id) => setSlideBook((book) => closeTab(book, id, defaultSlideForm))}
                />
                <SlideForm
                  key={slideTab.id}
                  simpleMode={simpleMode}
                  state={slideTab.form}
                  onState={(next) => setSlideBook((book) => patchTab(book, slideTab.id, (tab) => ({ form: next(tab.form) })))}
                  job={slideTab.job}
                  onJob={(job) => setSlideBook((book) => patchTab(book, slideTab.id, () => ({ job })))}
                  onMessage={showMessage}
                  onGoTemplates={() => void transitionToPage('templates')}
                  onOpenWorkbench={(assetId) => void openWorkbench(assetId)}
                  t={t}
                  d={d}
                />
              </div>
            </div>
          )}
          {visited.has('templates') && (
            <div className={paneClass('templates', page, supportsViewTransitions)}>
              <TemplateLibrary t={d} onMessage={showMessage} />
            </div>
          )}
          {visited.has('history') && (
            <div className={paneClass('history', page, supportsViewTransitions)}>
              <HistoryPage
                d={d}
                onOpen={openJob}
                onDelete={(job, refresh) => deleteSettled(job, refresh)}
                onRerun={rerunJob}
                onOpenWorkbench={(assetId) => void openWorkbench(assetId)}
              />
            </div>
          )}
          {visited.has('workbench') && (
            <div className={paneClass('workbench', page, supportsViewTransitions)}>
              <WorkbenchPage
                lang={lang}
                request={workbenchRequest}
                onRequestHandled={handleWorkbenchRequest}
                onMessage={showMessage}
                registerLeaveGuard={registerWorkbenchLeaveGuard}
              />
            </div>
          )}
          {visited.has('settings') && (
            <div className={paneClass('settings', page, supportsViewTransitions)}>
              <SettingsPane>
                <Settings
                  config={config}
                  onChange={setConfig}
                  onSave={persistConfig}
                  t={t}
                  tail={
                    <SettingsSection
                      title={d.nav.workbench}
                      summary={workbenchSummary.summary}
                      badge={workbenchSummary.badge}
                      badgeTone={workbenchSummary.badgeTone}
                      open={workbenchOpen}
                      onToggle={() => setWorkbenchOpen((value) => !value)}
                    >
                      <WorkbenchSettings lang={lang} onMessage={showMessage} embedded onSummary={setWorkbenchSummary} />
                    </SettingsSection>
                  }
                />
              </SettingsPane>
            </div>
          )}
          {visited.has('about') && (
            <div className={paneClass('about', page, supportsViewTransitions)}>
              <AboutPage
                d={d}
                version={packageMetadata.version}
                autoCheck={autoUpdate}
                update={update}
                onAutoCheck={changeAutoUpdate}
                onCheck={() => void runUpdateCheck()}
                onOpen={(url) => void openUrl(url)}
              />
            </div>
          )}
        </div>
      </main>

      {toast && (
        <div className={`toast toast-${toast.tone}`} role="status" aria-live="polite" key={toast.id}>
          <span>{toast.text}</span>
          <button type="button" className="toast-close" aria-label="Close" onClick={() => setToast(null)}>
            ×
          </button>
        </div>
      )}
    </div>
  );
}

/**
 * Polls every unsettled job in a book, not only the visible tab's: a tab left
 * running in the background must still show its result when the user returns.
 * One interval for the whole book keeps the IPC rate flat as tabs are added.
 */
function useBookPolling<F>(
  book: TaskBook<F>,
  setBook: (next: (book: TaskBook<F>) => TaskBook<F>) => void,
  showMessage: (text: string, tone?: 'info' | 'error') => void
) {
  const live = liveJobs(book).map((job) => job.id);
  const liveKey = live.join(',');
  useEffect(() => {
    if (!liveKey) return;
    const ids = liveKey.split(',');
    const timer = window.setInterval(() => {
      for (const id of ids) {
        getJob(id)
          .then((job) => setBook((current) => applyJob(current, job)))
          .catch((error) => showMessage(error instanceof Error ? error.message : String(error), 'error'));
      }
    }, 1800);
    return () => window.clearInterval(timer);
  }, [liveKey, setBook, showMessage]);
}

// Restores a stored figure payload into a form. Unknown or missing fields
// fall back to the given base so a partial payload still yields a valid form.
// 约束里由字体/背景色选项生成的那两句还原回下拉与色块，不然重开历史任务时
// 选项会显示成「默认字体」和空色块，而文本里却留着那两句话。
export function figureFormFrom(payload: unknown, base: FigureFormState, t: DesktopCopy['style']): FigureFormState {
  if (!payload || typeof payload !== 'object') return base;
  const p = payload as Record<string, unknown>;
  const ids = Array.isArray(p.template_ids)
    ? (p.template_ids as unknown[]).filter((id): id is string => typeof id === 'string')
    : [];
  const rules = typeof p.custom_prompt === 'string' ? p.custom_prompt : '';
  const { custom, choice } = splitRules(rules, t);
  return {
    ...base,
    title: typeof p.figure_title === 'string' ? p.figure_title : base.title,
    description: typeof p.section_description === 'string' ? p.section_description : base.description,
    selected: ids.length > 0 ? ids : base.selected,
    aspectRatio: typeof p.aspect_ratio === 'string' ? p.aspect_ratio : base.aspectRatio,
    layoutFidelity:
      p.layout_fidelity === 'strict' || p.layout_fidelity === 'balanced' || p.layout_fidelity === 'loose'
        ? p.layout_fidelity
        : base.layoutFidelity,
    styleStrength:
      p.style_strength === 'high' || p.style_strength === 'medium' || p.style_strength === 'low'
        ? p.style_strength
        : base.styleStrength,
    custom,
    style: choice
  };
}

export function slideFormFrom(payload: unknown, base: SlideFormState, t: DesktopCopy['style']): SlideFormState {
  if (!payload || typeof payload !== 'object') return base;
  const p = payload as Record<string, unknown>;
  const rules = typeof p.custom_prompt === 'string' ? p.custom_prompt : '';
  const { custom, choice } = splitRules(rules, t);
  return {
    ...base,
    material: typeof p.material_text === 'string' ? p.material_text : base.material,
    pages: typeof p.page_count === 'number' && p.page_count > 0 ? p.page_count : base.pages,
    custom,
    style: choice
  };
}

function TaskStrip<F>({
  book,
  d,
  formTitle,
  onSelect,
  onAdd,
  onClose
}: {
  book: TaskBook<F>;
  d: DesktopCopy;
  formTitle: (form: F) => string;
  onSelect: (id: string) => void;
  onAdd: () => void;
  onClose: (id: string) => void;
}) {
  return (
    <div className="task-strip" role="tablist" aria-label={d.tasks.label}>
      {book.tabs.map((tab) => {
        const active = tab.id === book.active;
        const status = tab.job?.status;
        return (
          <div
            key={tab.id}
            role="tab"
            aria-selected={active}
            tabIndex={0}
            className={`task-tab${active ? ' active' : ''}`}
            onClick={() => onSelect(tab.id)}
            onKeyDown={(event) => {
              if (event.key === 'Enter' || event.key === ' ') {
                event.preventDefault();
                onSelect(tab.id);
              }
            }}
          >
            {status && <span className={`task-tab-dot ${status}`} aria-hidden="true" />}
            <span className="task-tab-label">{tabLabel(tab, formTitle, d.tasks.untitled)}</span>
            <button
              type="button"
              className="task-tab-close"
              aria-label={d.tasks.close}
              title={d.tasks.close}
              onClick={(event) => {
                event.stopPropagation();
                onClose(tab.id);
              }}
            >
              ×
            </button>
          </div>
        );
      })}
      <button
        type="button"
        className="task-tab-add"
        aria-label={d.tasks.add}
        title={book.tabs.length >= MAX_TABS ? d.tasks.full(MAX_TABS) : d.tasks.add}
        disabled={book.tabs.length >= MAX_TABS}
        onClick={onAdd}
      >
        +
      </button>
    </div>
  );
}

// Cards are served a downscaled variant (`?w=`); the lightbox drops the query
// to show the untouched image, one at a time and only when asked for.
function fullSize(url: string): string {
  const cut = url.indexOf('?');
  return cut === -1 ? url : url.slice(0, cut);
}

export const HistoryCard = memo(function HistoryCard({
  job,
  d,
  filteredOut,
  confirming,
  onOpen,
  onPreview,
  onDelete,
  onRerun,
  onOpenWorkbench
}: {
  job: JobRecord;
  d: DesktopCopy;
  filteredOut: boolean;
  confirming: boolean;
  onOpen: (job: JobRecord) => void;
  onPreview: (url: string) => void;
  onDelete: (job: JobRecord) => void;
  onRerun: (job: JobRecord) => void;
  onOpenWorkbench: (assetId: string) => void;
}) {
  // 简单模式的预览是所选母版：母版被删或挪走时退回原来那个空图标，
  // 而不是留一张碎图。
  const [previewBroken, setPreviewBroken] = useState(false);
  const preview = previewBroken ? null : job.thumbnail;
  const pill = (() => {
    switch (job.status) {
      case 'queued':
      case 'running':
        return d.recent.running;
      case 'succeeded':
        return d.recent.done;
      case 'failed':
        return d.recent.failed;
      default:
        return d.history.stopped;
    }
  })();
  return (
    <li className={`history-card${filteredOut ? ' is-hidden' : ''}`}>
      <button
        type="button"
        className="hc-image"
        onClick={() => (preview ? onPreview(fullSize(preview)) : onOpen(job))}
        title={
          preview
            ? job.simple
              ? `${d.history.reference}\n${d.history.simpleHint}`
              : d.history.zoom
            : job.title ?? job.message ?? job.id
        }
      >
        {preview ? (
          <img
            src={preview}
            alt=""
            decoding="async"
            onError={() => setPreviewBroken(true)}
          />
        ) : (
          <span className="hc-image-empty">
            {job.mode === 'ppt_slide' ? <IconSlide /> : <IconFigure />}
          </span>
        )}
      </button>
      <div className="hc-body">
        <button
          type="button"
          className="hc-open"
          onClick={() => onOpen(job)}
          title={job.title ?? job.message ?? job.id}
        >
          <span className="hc-title-row">
            <span className={`recent-pill recent-pill-${job.status}`}>{pill}</span>
            {job.simple && (
              <span className="recent-pill hc-simple" title={d.history.simpleHint}>
                {d.history.simple}
              </span>
            )}
            {job.rating && (
              <span className={`recent-pill hc-rating hc-rating-${job.rating}`} title={d.history.ratingTitle}>
                {d.history.rating[job.rating]}
              </span>
            )}
            <span className="hc-title" title={job.title ?? undefined}>
              {job.title || d.recent.untitled}
            </span>
          </span>
          <span className={`recent-summary recent-summary-${job.status}`}>
            {recentSummary(job, d)}
          </span>
        </button>
        <div className="hc-footer">
          <span className="desktop-recent-time">{shortTime(job.created_at)}</span>
          {isJobSettled(job.status) && (
            <span className="hc-actions">
              {job.images[0]?.asset_id && (
                <button
                  type="button"
                  className="recent-workbench"
                  aria-label={d.nav.workbench}
                  title={d.nav.workbench}
                  onClick={() => onOpenWorkbench(job.images[0].asset_id!)}
                >
                  ✎
                </button>
              )}
              <button
                type="button"
                className="recent-rerun"
                aria-label={d.recent.rerun}
                title={d.recent.rerun}
                onClick={() => onRerun(job)}
              >
                ⟳
              </button>
              <button
                type="button"
                className={`recent-delete${confirming ? ' confirming' : ''}`}
                aria-label={d.recent.delete}
                title={d.recent.delete}
                onClick={() => onDelete(job)}
              >
                {confirming ? d.recent.confirmDelete : '×'}
              </button>
            </span>
          )}
        </div>
      </div>
    </li>
  );
});

const HISTORY_PAGE_SIZE = 50;

// Rows survive page switches so re-entering history paints the previous grid
// immediately instead of flashing empty while the query round-trips.
const historyCache = new Map<number, JobRecord[]>();

// listJobs hands back fresh objects every poll. Reusing the previous object for
// rows that did not change keeps HistoryCard's memo effective, so a 3s refresh
// no longer re-renders every card — and never re-decodes their images.
function reconcile(previous: JobRecord[], next: JobRecord[]): JobRecord[] {
  const byId = new Map(previous.map((job) => [job.id, job]));
  let changed = previous.length !== next.length;
  const merged = next.map((job, index) => {
    const old = byId.get(job.id);
    const reusable = old && old.updated_at === job.updated_at && old.status === job.status;
    if (!reusable || previous[index]?.id !== job.id) changed = true;
    return reusable ? old : job;
  });
  return changed ? merged : previous;
}

export function HistoryPage({
  d,
  onOpen,
  onDelete,
  onRerun,
  onOpenWorkbench
}: {
  d: DesktopCopy;
  onOpen: (job: JobRecord) => void;
  onDelete: (job: JobRecord, refresh: () => void) => void;
  onRerun: (job: JobRecord) => void;
  onOpenWorkbench: (assetId: string) => void;
}) {
  const [pageIndex, setPageIndex] = useState(0);
  const [rows, setRows] = useState<JobRecord[]>(() => historyCache.get(0) ?? []);
  const [modeFilter, setModeFilter] = useState<'all' | 'paper_figure' | 'ppt_slide'>('all');
  const [statusFilter, setStatusFilter] = useState<'all' | 'live' | 'succeeded' | 'failed'>('all');
  const [simpleFilter, setSimpleFilter] = useState<'all' | 'normal' | 'simple'>('all');
  const [query, setQuery] = useState('');
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);

  useEffect(() => {
    if (!previewUrl) return;
    function onKey(event: KeyboardEvent) {
      if (event.key === 'Escape') setPreviewUrl(null);
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [previewUrl]);

  useEffect(() => {
    if (!confirmDeleteId) return;
    const timer = window.setTimeout(() => setConfirmDeleteId(null), 3000);
    return () => window.clearTimeout(timer);
  }, [confirmDeleteId]);

  const refresh = useCallback(() => {
    return listJobs(HISTORY_PAGE_SIZE, pageIndex * HISTORY_PAGE_SIZE)
      .then((jobs) => {
        historyCache.set(pageIndex, jobs);
        setRows((current) => reconcile(current, jobs));
      })
      .catch(() => {});
  }, [pageIndex]);

  useEffect(() => {
    // Show whatever this page last held, then reconcile against the server.
    setRows(historyCache.get(pageIndex) ?? []);
    let cancelled = false;
    listJobs(HISTORY_PAGE_SIZE, pageIndex * HISTORY_PAGE_SIZE)
      .then((jobs) => {
        historyCache.set(pageIndex, jobs);
        if (!cancelled) setRows((current) => reconcile(current, jobs));
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [pageIndex]);

  // The rail popup used to poll live jobs; without it, the history page
  // keeps that duty while anything is queued or running.
  const currentLive = rows.some(
    (job) => job.status === 'queued' || job.status === 'running'
  );
  useEffect(() => {
    if (!currentLive) return;
    const timer = window.setInterval(refresh, 3000);
    return () => window.clearInterval(timer);
  }, [currentLive, refresh]);

  // Cards take the job as an argument so these stay referentially stable and
  // the memo above actually holds across polls and filter changes.
  const confirmRef = useRef<string | null>(null);
  useEffect(() => {
    confirmRef.current = confirmDeleteId;
  }, [confirmDeleteId]);

  const handleDelete = useCallback(
    (job: JobRecord) => {
      if (confirmRef.current !== job.id) {
        setConfirmDeleteId(job.id);
        return;
      }
      setConfirmDeleteId(null);
      onDelete(job, refresh);
    },
    [onDelete, refresh]
  );

  const matches = (job: JobRecord) => {
    if (modeFilter !== 'all' && job.mode !== modeFilter) return false;
    if (statusFilter === 'live' && isJobSettled(job.status)) return false;
    if (statusFilter === 'succeeded' && job.status !== 'succeeded') return false;
    if (statusFilter === 'failed' && job.status !== 'failed') return false;
    if (simpleFilter === 'normal' && job.simple) return false;
    if (simpleFilter === 'simple' && !job.simple) return false;
    if (query.trim()) {
      const haystack = `${job.title ?? ''} ${job.message ?? ''}`.toLowerCase();
      if (!haystack.includes(query.trim().toLowerCase())) return false;
    }
    return true;
  };
  const visibleCount = rows.reduce((total, job) => total + (matches(job) ? 1 : 0), 0);

  const groupOf = (iso: string) => {
    const date = new Date(iso);
    const now = new Date();
    const dayStart = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const diffDays = Math.round((dayStart(now) - dayStart(date)) / 86_400_000);
    if (diffDays <= 0) return d.history.today;
    if (diffDays === 1) return d.history.yesterday;
    return d.history.earlier;
  };

  // Every row is grouped and rendered, matching or not, and a filter only
  // toggles visibility. Dropping non-matching cards from the tree instead would
  // destroy their <img>, and remounting one re-requests and re-decodes the
  // image — a burst of that on each filter click is what made the bar stutter.
  const grouped: Array<[string, JobRecord[]]> = [];
  for (const job of rows) {
    const label = groupOf(job.created_at);
    const bucket = grouped.find(([key]) => key === label);
    if (bucket) {
      bucket[1].push(job);
    } else {
      grouped.push([label, [job]]);
    }
  }

  const seg = (
    options: Array<{ key: string; label: string }>,
    value: string,
    onPick: (key: string) => void,
    label: string
  ) => (
    <div className="history-seg" role="group" aria-label={label}>
      {options.map((option) => (
        <button
          key={option.key}
          type="button"
          className={`history-seg-btn${value === option.key ? ' active' : ''}`}
          onClick={() => onPick(option.key)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );

  return (
    <section className="clay-panel history-panel">
      <div className="history-toolbar">
        {seg(
          [
            { key: 'all', label: d.history.all },
            { key: 'paper_figure', label: d.nav.paper },
            { key: 'ppt_slide', label: d.nav.ppt }
          ],
          modeFilter,
          (key) => setModeFilter(key as typeof modeFilter),
          d.history.filterMode
        )}
        {seg(
          [
            { key: 'all', label: d.history.all },
            { key: 'normal', label: d.history.normal },
            { key: 'simple', label: d.history.simple }
          ],
          simpleFilter,
          (key) => setSimpleFilter(key as typeof simpleFilter),
          d.history.filterSimple
        )}
        {seg(
          [
            { key: 'all', label: d.history.all },
            { key: 'live', label: d.recent.running },
            { key: 'succeeded', label: d.recent.done },
            { key: 'failed', label: d.recent.failed }
          ],
          statusFilter,
          (key) => setStatusFilter(key as typeof statusFilter),
          d.history.filterStatus
        )}
        <input
          className="history-search"
          placeholder={d.history.searchPlaceholder}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <span className="history-count">{d.history.count(visibleCount)}</span>
      </div>
      {visibleCount === 0 && (
        <p className="rail-pop-empty history-empty">{d.history.emptyFiltered}</p>
      )}
      <div className="history-groups">
        {grouped.map(([label, jobs]) => {
          const shown = jobs.reduce((total, job) => total + (matches(job) ? 1 : 0), 0);
          return (
            <section key={label} className={`history-group${shown === 0 ? ' is-hidden' : ''}`}>
              <h3 className="history-group-title">
                {label}
                <span>{shown}</span>
              </h3>
              <ul className="history-list">
                {jobs.map((job) => (
                  <HistoryCard
                    key={job.id}
                    job={job}
                    d={d}
                    filteredOut={!matches(job)}
                    confirming={confirmDeleteId === job.id}
                    onOpen={onOpen}
                    onPreview={setPreviewUrl}
                    onDelete={handleDelete}
                    onRerun={onRerun}
                    onOpenWorkbench={onOpenWorkbench}
                  />
                ))}
              </ul>
            </section>
          );
        })}
      </div>
      <div className="history-pager">
        <button
          type="button"
          className="history-page-btn"
          disabled={pageIndex === 0}
          onClick={() => setPageIndex((index) => Math.max(0, index - 1))}
        >
          {d.history.prev}
        </button>
        <span className="history-page-index">{pageIndex + 1}</span>
        <button
          type="button"
          className="history-page-btn"
          disabled={rows.length < HISTORY_PAGE_SIZE}
          onClick={() => setPageIndex((index) => index + 1)}
        >
          {d.history.next}
        </button>
      </div>
      {previewUrl && (
        <div
          className="lightbox"
          role="dialog"
          aria-label={d.history.zoom}
          onClick={() => setPreviewUrl(null)}
        >
          <button
            type="button"
            className="lightbox-close"
            aria-label={d.history.closePreview}
            onClick={() => setPreviewUrl(null)}
          >
            ×
          </button>
          <img src={previewUrl} alt="" onClick={(event) => event.stopPropagation()} />
        </div>
      )}
    </section>
  );
}

const pageHead: Record<
  Page,
  { title: (t: Copy, d: DesktopCopy) => string; intro: (t: Copy, d: DesktopCopy) => string }
> = {
  paper: { title: (t) => t.paper.title, intro: (t) => t.paper.intro },
  ppt: { title: (t) => t.ppt.title, intro: (t) => t.ppt.intro },
  templates: { title: (_t, d) => d.templates.title, intro: (_t, d) => d.templates.intro },
  history: { title: (_t, d) => d.history.title, intro: (_t, d) => d.history.intro },
  settings: { title: (_t, d) => d.nav.settings, intro: (_t, d) => d.settingsIntro },
  workbench: { title: (_t, d) => d.nav.workbench, intro: (_t, d) => d.workbenchIntro },
  about: { title: (_t, d) => d.about.title, intro: (_t, d) => d.about.intro }
};

function IconFigure() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.75" y="3.25" width="14.5" height="13.5" rx="2.5" />
      <path d="M2.75 12.5l3.6-3.4 2.9 2.6 3-3.4 4.1 4" strokeLinecap="round" strokeLinejoin="round" />
      <circle cx="7" cy="7" r="1.1" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconSlide() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.5" y="3.75" width="15" height="10.5" rx="2" />
      <path d="M10 14.25v2.5M7 16.75h6" strokeLinecap="round" />
    </svg>
  );
}

function IconTemplates() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="2.75" y="2.75" width="6" height="6" rx="1.8" />
      <rect x="11.25" y="2.75" width="6" height="6" rx="1.8" />
      <rect x="2.75" y="11.25" width="6" height="6" rx="1.8" />
      <rect x="11.25" y="11.25" width="6" height="6" rx="1.8" />
    </svg>
  );
}

function IconTheme({ theme }: { theme: Theme }) {
  return theme === 'dark' ? (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <circle cx="10" cy="10" r="3.2" />
      <path d="M10 2.2v1.5M10 16.3v1.5M2.2 10h1.5M16.3 10h1.5M4.5 4.5l1.1 1.1M14.4 14.4l1.1 1.1M15.5 4.5l-1.1 1.1M5.6 14.4l-1.1 1.1" strokeLinecap="round" />
    </svg>
  ) : (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <path d="M16.5 12.3A6.7 6.7 0 0 1 7.7 3.5a6.7 6.7 0 1 0 8.8 8.8Z" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function IconGear() {
  return (
    <svg viewBox="0 0 20 20" aria-hidden="true">
      <path
        fill="currentColor"
        fillRule="evenodd"
        clipRule="evenodd"
        d="M8.94 1.5h2.12c.5 0 .92.36 1 .85l.2 1.2c.42.15.82.35 1.18.6l1.14-.44a1.01 1.01 0 0 1 1.23.43l1.06 1.84c.25.43.16.98-.22 1.3l-.94.78c.04.23.06.47.06.71 0 .24-.02.48-.06.71l.94.78c.38.32.47.87.22 1.3l-1.06 1.84a1.01 1.01 0 0 1-1.23.43l-1.14-.43c-.36.24-.76.44-1.18.59l-.2 1.2a1.01 1.01 0 0 1-1 .85H8.94a1.01 1.01 0 0 1-1-.85l-.2-1.2a5.9 5.9 0 0 1-1.18-.6l-1.14.44a1.01 1.01 0 0 1-1.23-.43L3.13 12.1a1.01 1.01 0 0 1 .22-1.3l.94-.78A5.6 5.6 0 0 1 4.23 10c0-.24.02-.48.06-.71l-.94-.78a1.01 1.01 0 0 1-.22-1.3l1.06-1.84a1.01 1.01 0 0 1 1.23-.43l1.14.43c.36-.24.76-.44 1.18-.59l.2-1.2c.08-.49.5-.85 1-.85Zm1.06 5.55a2.95 2.95 0 1 0 0 5.9 2.95 2.95 0 0 0 0-5.9Z"
      />
    </svg>
  );
}

function IconWorkbench() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <rect x="3" y="3" width="14" height="14" rx="2.2" />
      <path d="M6 7h8M6 10h5M6 13h3" strokeLinecap="round" />
      <circle cx="14" cy="13" r="1.2" fill="currentColor" stroke="none" />
    </svg>
  );
}

function IconHistory() {
  return (
    <svg viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      <path d="M3.5 4.5v11a1.5 1.5 0 0 0 1.5 1.5h10a1.5 1.5 0 0 0 1.5-1.5v-7a1.5 1.5 0 0 0-1.5-1.5H9.6L8 4.5H5Z" strokeLinejoin="round" />
      <path d="M3.5 8h12.5" strokeLinecap="round" />
    </svg>
  );
}


function shortTime(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  const today = new Date();
  const sameDay =
    date.getFullYear() === today.getFullYear() &&
    date.getMonth() === today.getMonth() &&
    date.getDate() === today.getDate();
  const pad = (value: number) => String(value).padStart(2, '0');
  return sameDay
    ? `${pad(date.getHours())}:${pad(date.getMinutes())}`
    : `${pad(date.getMonth() + 1)}/${pad(date.getDate())}`;
}

function recentSummary(job: JobRecord, d: DesktopCopy): string {
  const mode = job.mode === 'ppt_slide' ? d.nav.ppt : d.nav.paper;
  const message = (job.message ?? '').trim();
  const firstLine = message.split('\n')[0]?.trim() ?? '';
  if (job.status === 'succeeded') {
    const finished = shortTime(job.updated_at);
    return finished ? `${mode} · ${d.recent.finishedAt} ${finished}` : mode;
  }
  if (job.status === 'failed') {
    const clipped = firstLine.length > 60 ? `${firstLine.slice(0, 60)}…` : firstLine;
    return clipped ? `${mode} · ${clipped}` : `${mode} · ${d.recent.failed}`;
  }
  if (firstLine) {
    return `${mode} · ${firstLine}`;
  }
  return `${mode} · ${job.status === 'queued' || job.status === 'running' ? d.recent.running : d.recent.done}`;
}
