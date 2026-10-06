import { useEffect, useId, useState, type ReactNode } from 'react';
import { createJob, listTemplates, uploadAsset } from '../api';
import { JobPanel, copy, type Lang } from '../app';
import type { AssetUpload, JobRecord, TemplateSummary } from '../types';
import type { DesktopCopy } from './copy';
import { composeRules, defaultStyleChoice, StyleConstraints, type StyleChoice } from './style_picker';

type Copy = (typeof copy)[Lang];

const MAX_TEMPLATES = 3;

function Card({
  title,
  aside,
  foot,
  children,
  flush
}: {
  title?: ReactNode;
  aside?: ReactNode;
  foot?: ReactNode;
  children: ReactNode;
  flush?: boolean;
}) {
  return (
    <section className="dp-card">
      {(title || aside) && (
        <header className="dp-card-head">
          {typeof title === 'string' ? <h2>{title}</h2> : title}
          {aside ? <div className="dp-card-head-aside">{aside}</div> : null}
        </header>
      )}
      <div className={`dp-card-body${flush ? ' flush' : ''}`}>{children}</div>
      {foot ? <footer className="dp-card-foot">{foot}</footer> : null}
    </section>
  );
}

function Seg<T extends string>({
  value,
  items,
  onChange
}: {
  value: T;
  items: Array<{ key: T; label: string; dot?: boolean }>;
  onChange: (key: T) => void;
}) {
  return (
    <div className="dp-seg" role="tablist">
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="tab"
          aria-selected={value === item.key}
          className={value === item.key ? 'active' : undefined}
          onClick={() => onChange(item.key)}
        >
          {item.label}
          {item.dot && <span className="dp-seg-dot" aria-hidden="true" />}
        </button>
      ))}
    </div>
  );
}

function DField({
  label,
  children,
  span
}: {
  label: string;
  children: ReactNode;
  span?: boolean;
}) {
  return (
    <label className={`dp-field${span ? ' span' : ''}`}>
      <span className="dp-field-label">{label}</span>
      {children}
    </label>
  );
}

function DPick({
  accept,
  label,
  value,
  multiple,
  onChange,
  children
}: {
  accept: string;
  label: string;
  value: string;
  multiple?: boolean;
  onChange: (files: FileList | null) => void;
  children?: ReactNode;
}) {
  const id = useId();
  return (
    <div className="dp-pick">
      <input id={id} type="file" accept={accept} multiple={multiple} onChange={(event) => onChange(event.target.files)} />
      <label htmlFor={id}>{label}</label>
      {children ??
        (value ? (
          <span className="dp-pick-name" title={value}>
            {value}
          </span>
        ) : null)}
    </div>
  );
}

function GrowField({
  label,
  hint,
  value,
  onChange,
  grow = 1,
  aside
}: {
  label: string;
  hint?: string;
  value: string;
  onChange: (value: string) => void;
  grow?: number;
  /** Controls shown at the right end of the label row (not inside the label). */
  aside?: ReactNode;
}) {
  const id = useId();
  return (
    <div className="dp-field span dp-field-grow" style={{ flexGrow: grow }}>
      <div className="dp-field-head">
        <label className="dp-field-label" htmlFor={id}>
          {label}
        </label>
        {aside}
      </div>
      <textarea
        id={id}
        className="dp-ta-fill"
        placeholder={hint}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
    </div>
  );
}

function DStep({
  value,
  min,
  max,
  onChange
}: {
  value: number;
  min: number;
  max: number;
  onChange: (value: number) => void;
}) {
  const clamp = (next: number) => Math.min(max, Math.max(min, next));
  return (
    <div className="dp-step">
      <button type="button" disabled={value <= min} aria-label="-" onClick={() => onChange(clamp(value - 1))}>
        −
      </button>
      <input
        type="number"
        min={min}
        max={max}
        value={value}
        onChange={(event) => {
          const next = Number(event.target.value);
          if (Number.isFinite(next)) onChange(clamp(next));
        }}
      />
      <button type="button" disabled={value >= max} aria-label="+" onClick={() => onChange(clamp(value + 1))}>
        +
      </button>
    </div>
  );
}

function TileGrid({
  items,
  selected,
  onToggle,
  empty
}: {
  items: TemplateSummary[];
  selected: string[];
  onToggle: (id: string) => void;
  empty: ReactNode;
}) {
  if (items.length === 0) return <div className="dp-empty">{empty}</div>;
  return (
    <div className="dp-tiles">
      {items.map((item) => {
        const on = selected.includes(item.id);
        return (
          <button
            key={item.id}
            type="button"
            className={`dp-tile${on ? ' on' : ''}`}
            aria-pressed={on}
            onClick={() => onToggle(item.id)}
            title={item.visual_intent || item.category || item.kind}
          >
            <img src={item.image_url} alt="" loading="lazy" />
            <span className="dp-tile-cap">{item.category || item.kind}</span>
            {on && <span className="dp-tile-mark" aria-hidden="true" />}
          </button>
        );
      })}
    </div>
  );
}

function ResultPane({
  job,
  t,
  emptyText,
  onJob,
  onMessage,
  onOpenWorkbench
}: {
  job: JobRecord | null;
  t: Copy;
  emptyText: string;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onOpenWorkbench?: (assetId: string) => void;
}) {
  return (
    <div className="result-stack">
      {job ? (
        <JobPanel
          job={job}
          t={t}
          onCancelled={(next) => {
            onJob(next);
            onMessage(t.result.stopped);
          }}
          onResumed={(next) => {
            onJob(next);
            onMessage(t.result.resuming);
          }}
          onError={(message) => onMessage(message, 'error')}
          onOpenWorkbench={onOpenWorkbench}
          onRated={onJob}
        />
      ) : (
        <div className="dp-empty">{emptyText}</div>
      )}
    </div>
  );
}

export type FigureFormState = {
  kind: string;
  query: string;
  selected: string[];
  title: string;
  description: string;
  aspectRatio: string;
  layoutFidelity: 'strict' | 'balanced' | 'loose';
  styleStrength: 'high' | 'medium' | 'low';
  custom: string;
  style: StyleChoice;
};

export const defaultFigureForm: FigureFormState = {
  kind: 'diagram',
  query: '',
  selected: [],
  title: '',
  description: '',
  aspectRatio: 'inherit',
  layoutFidelity: 'balanced',
  styleStrength: 'high',
  custom: '',
  style: defaultStyleChoice
};

export function FigureForm({
  simpleMode,
  state,
  onState,
  job,
  onJob,
  onMessage,
  onGoTemplates,
  onOpenWorkbench,
  t,
  d
}: {
  simpleMode: boolean;
  state: FigureFormState;
  onState: (next: (current: FigureFormState) => FigureFormState) => void;
  job: JobRecord | null;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onGoTemplates: () => void;
  onOpenWorkbench?: (assetId: string) => void;
  t: Copy;
  d: DesktopCopy;
}) {
  const [templates, setTemplates] = useState<TemplateSummary[]>([]);
  const [pane, setPane] = useState<'templates' | 'result'>('templates');
  const { kind, query, selected, title, description, aspectRatio, layoutFidelity, styleStrength, custom, style } = state;

  function patch(values: Partial<FigureFormState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    let cancelled = false;
    listTemplates(kind, query)
      .then((items) => {
        if (!cancelled) setTemplates(items);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [kind, query]);

  useEffect(() => {
    if (job) setPane('result');
  }, [job?.id]);

  function toggle(id: string) {
    onState((current) => ({
      ...current,
      selected: current.selected.includes(id)
        ? current.selected.filter((item) => item !== id)
        : [...current.selected, id].slice(0, MAX_TEMPLATES)
    }));
  }

  async function submit() {
    try {
      onMessage(t.paper.submitted);
      const created = await createJob({
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
          custom_prompt: composeRules(custom, style, d.style)
        }
      });
      onJob(created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  const ready = Boolean(title.trim() && description.trim() && selected.length > 0);

  return (
    <div className="dp-work">
      <Card
        title={d.pane.form}
        foot={
          <>
            <span className="dp-foot-note">{d.pane.selected(selected.length, MAX_TEMPLATES)}</span>
            <button type="button" className="dp-primary" disabled={!ready} onClick={submit}>
              {t.paper.generate}
            </button>
          </>
        }
      >
        <DField label={t.paper.figureTitle} span>
          <input value={title} onChange={(event) => patch({ title: event.target.value })} />
        </DField>
        <GrowField
          label={t.paper.description}
          value={description}
          onChange={(next) => patch({ description: next })}
          grow={2}
        />
        <div className="dp-row three">
          <DField label={t.paper.ratio}>
            <select value={aspectRatio} onChange={(event) => patch({ aspectRatio: event.target.value })}>
              <option value="inherit">{t.paper.inherited}</option>
              <option value="16:9">16:9</option>
              <option value="4:3">4:3</option>
              <option value="1:1">1:1</option>
              <option value="3:2">3:2</option>
            </select>
          </DField>
          <DField label={t.paper.fidelity}>
            <select
              value={layoutFidelity}
              onChange={(event) => patch({ layoutFidelity: event.target.value as FigureFormState['layoutFidelity'] })}
            >
              <option value="strict">strict</option>
              <option value="balanced">balanced</option>
              <option value="loose">loose</option>
            </select>
          </DField>
          <DField label={t.paper.strength}>
            <select
              value={styleStrength}
              onChange={(event) => patch({ styleStrength: event.target.value as FigureFormState['styleStrength'] })}
            >
              <option value="high">high</option>
              <option value="medium">medium</option>
              <option value="low">low</option>
            </select>
          </DField>
        </div>
        <GrowField
          label={t.paper.custom}
          hint={t.paper.customHint}
          value={custom}
          onChange={(next) => patch({ custom: next })}
          aside={<StyleConstraints t={d.style} value={style} onChange={(next) => patch({ style: next })} />}
        />
      </Card>

      <Card
        title={
          <Seg
            value={pane}
            onChange={setPane}
            items={[
              { key: 'templates', label: d.pane.templates },
              { key: 'result', label: d.pane.result, dot: Boolean(job) }
            ]}
          />
        }
        aside={
          pane === 'templates' ? (
            <div className="dp-head-tools">
              <select
                className="dp-mini"
                value={kind}
                onChange={(event) => patch({ kind: event.target.value })}
                aria-label={t.paper.kind}
              >
                <option value="diagram">{d.templates.diagram}</option>
                <option value="plot">{d.templates.plot}</option>
              </select>
              <input
                className="dp-mini dp-search"
                placeholder={d.pane.search}
                value={query}
                onChange={(event) => patch({ query: event.target.value })}
                aria-label={t.paper.search}
              />
            </div>
          ) : null
        }
        flush={pane === 'templates'}
      >
        {pane === 'templates' ? (
          <TileGrid
            items={templates}
            selected={selected}
            onToggle={toggle}
            empty={
              <>
                <strong>{d.pane.figureEmpty}</strong>
                <p>{d.pane.emptyHint}</p>
                <button type="button" className="dp-ghost" onClick={onGoTemplates}>
                  {d.pane.goTemplates}
                </button>
              </>
            }
          />
        ) : (
          <ResultPane
            job={job}
            t={t}
            emptyText={d.pane.resultEmpty}
            onJob={onJob}
            onMessage={onMessage}
            onOpenWorkbench={onOpenWorkbench}
          />
        )}
      </Card>
    </div>
  );
}

export type SlideFormState = {
  master: TemplateSummary | null;
  materials: AssetUpload[];
  material: string;
  pages: number;
  custom: string;
  style: StyleChoice;
};

export const defaultSlideForm: SlideFormState = {
  master: null,
  materials: [],
  material: '',
  pages: 1,
  custom: '',
  style: defaultStyleChoice
};

export function SlideForm({
  simpleMode,
  state,
  onState,
  job,
  onJob,
  onMessage,
  onGoTemplates,
  onOpenWorkbench,
  t,
  d
}: {
  simpleMode: boolean;
  state: SlideFormState;
  onState: (next: (current: SlideFormState) => SlideFormState) => void;
  job: JobRecord | null;
  onJob: (job: JobRecord) => void;
  onMessage: (text: string, tone?: 'info' | 'error') => void;
  onGoTemplates: () => void;
  onOpenWorkbench?: (assetId: string) => void;
  t: Copy;
  d: DesktopCopy;
}) {
  const [masters, setMasters] = useState<TemplateSummary[]>([]);
  const [query, setQuery] = useState('');
  const [pane, setPane] = useState<'master' | 'result'>('master');
  const { master, materials, material, pages, custom, style } = state;

  function patch(values: Partial<SlideFormState>) {
    onState((current) => ({ ...current, ...values }));
  }

  useEffect(() => {
    let cancelled = false;
    listTemplates('master', query)
      .then((items) => {
        if (!cancelled) setMasters(items);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [query]);

  useEffect(() => {
    if (job) setPane('result');
  }, [job?.id]);

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
    if (!master) return;
    try {
      onMessage(t.ppt.submitted);
      const created = await createJob({
        mode: 'ppt_slide',
        simple_mode: simpleMode,
        payload: {
          template_id: master.id,
          material_text: material,
          material_asset_ids: materials.map((item) => item.id),
          page_count: pages,
          custom_prompt: composeRules(custom, style, d.style)
        }
      });
      onJob(created);
    } catch (error) {
      onMessage(error instanceof Error ? error.message : t.common.submitFailed, 'error');
    }
  }

  const ready = Boolean(master && (material.trim() || materials.length > 0));
  const materialLabel = materials.length ? `${t.common.uploadedFiles} ${materials.length}` : '';

  return (
    <div className="dp-work">
      <Card
        title={d.pane.form}
        foot={
          <>
            <span className="dp-foot-note">{master ? master.category || master.kind : d.pane.masterNone}</span>
            <button type="button" className="dp-primary" disabled={!ready} onClick={submit}>
              {t.ppt.generate}
            </button>
          </>
        }
      >
        <div className="dp-row two">
          <DField label={t.ppt.pages}>
            <DStep value={pages} min={1} max={20} onChange={(next) => patch({ pages: next })} />
          </DField>
          <DField label={t.ppt.materialFile}>
            <DPick
              accept=".pdf,.doc,.docx,.txt,.md,.markdown,.csv,.tsv,.json,application/pdf,text/*"
              label={t.common.choose}
              value={materialLabel}
              multiple
              onChange={onMaterialFiles}
            >
              {materials.length > 0 ? (
                <div className="dp-chips in-pick">
                  {materials.map((item) => (
                    <span key={item.id} className="dp-chip" title={item.filename}>
                      <span className="dp-chip-name">{item.filename}</span>
                      <button
                        type="button"
                        className="dp-chip-x"
                        aria-label={`${d.pane.removeFile} ${item.filename}`}
                        title={d.pane.removeFile}
                        onClick={() =>
                          onState((current) => ({
                            ...current,
                            materials: current.materials.filter((entry) => entry.id !== item.id)
                          }))
                        }
                      >
                        <svg viewBox="0 0 12 12" aria-hidden="true">
                          <path d="M3.4 3.4 8.6 8.6M8.6 3.4 3.4 8.6" />
                        </svg>
                      </button>
                    </span>
                  ))}
                </div>
              ) : null}
            </DPick>
          </DField>
        </div>

        <GrowField
          label={t.ppt.material}
          hint={t.ppt.materialHint}
          value={material}
          onChange={(next) => patch({ material: next })}
          grow={2}
        />
        <GrowField
          label={t.ppt.custom}
          hint={t.ppt.customHint}
          value={custom}
          onChange={(next) => patch({ custom: next })}
          aside={<StyleConstraints t={d.style} value={style} onChange={(next) => patch({ style: next })} />}
        />
      </Card>

      <Card
        title={
          <Seg
            value={pane}
            onChange={setPane}
            items={[
              { key: 'master', label: d.pane.master },
              { key: 'result', label: d.pane.result, dot: Boolean(job) }
            ]}
          />
        }
        aside={
          pane === 'master' ? (
            <div className="dp-head-tools">
              <span className="dp-foot-note">{d.pane.count(masters.length)}</span>
              <input
                className="dp-mini dp-search"
                placeholder={d.pane.search}
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                aria-label={d.pane.search}
              />
            </div>
          ) : null
        }
        flush={pane === 'master'}
      >
        {pane === 'master' ? (
          <TileGrid
            items={masters}
            selected={master ? [master.id] : []}
            onToggle={(id) =>
              patch({ master: master?.id === id ? null : masters.find((item) => item.id === id) ?? null })
            }
            empty={
              <>
                <strong>{d.pane.masterEmpty}</strong>
                <p>{d.pane.emptyHint}</p>
                <button type="button" className="dp-ghost" onClick={onGoTemplates}>
                  {d.pane.goTemplates}
                </button>
              </>
            }
          />
        ) : (
          <ResultPane
            job={job}
            t={t}
            emptyText={d.pane.resultEmpty}
            onJob={onJob}
            onMessage={onMessage}
            onOpenWorkbench={onOpenWorkbench}
          />
        )}
      </Card>
    </div>
  );
}

export function SettingsPane({ children }: { children: ReactNode }) {
  return (
    <div className="dp-work single">
      <Card>{children}</Card>
    </div>
  );
}

export { Card as DesktopCard, Seg as DesktopSeg, DField as DesktopField, DPick as DesktopPick };
