// @vitest-environment jsdom
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// 设置页会顺带 import 一堆 Tauri API，这里只要它们不抛错。
vi.mock('./api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./api')>()),
  desktopAvailable: () => true
}));

import { copy, Settings } from './app';
import type { AppConfig, ModelProfile } from './types';

const t = copy.zh;

function profile(values: Partial<ModelProfile>): ModelProfile {
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
    output_defaults: { max_results: '3' },
    ...values
  };
}

function configFor(search: Partial<ModelProfile>): AppConfig {
  return {
    version: 1,
    active_design_profile: 'design-default',
    active_implement_profile: 'implement-default',
    active_search_profile: 'search-default',
    model_profiles: [
      profile({ id: 'design-default', role: 'design', protocol: 'openai_chat' }),
      profile({ id: 'implement-default', role: 'implement', protocol: 'image2' }),
      profile(search)
    ]
  };
}

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement('div');
  document.body.append(container);
  root = createRoot(container);
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

function render(config: AppConfig) {
  act(() => {
    root.render(createElement(Settings, { config, onChange: () => {}, onSave: () => {}, t }));
  });
}

function searchSection(): Element {
  const sections = [...container.querySelectorAll('.settings-section')];
  const found = sections.find(
    (section) => section.querySelector('.settings-section-title')?.textContent === t.settings.search
  );
  if (!found) throw new Error('search section missing');
  return found;
}

/** 该区块当前显示的字段：标签 -> 第一个控件里的值。 */
function searchFields(): Record<string, string> {
  const fields = [...searchSection().querySelectorAll('.field')];
  return Object.fromEntries(
    fields.map((field) => [
      field.querySelector('.field-label')?.textContent?.trim() ?? '',
      (field.querySelector('input, select') as HTMLInputElement | HTMLSelectElement | null)?.value ?? ''
    ])
  );
}

describe('设置页的搜索协议表单', () => {
  it('duckduckgo 只显示它会用到的字段：端点写死，地址/模型/密钥都不显示', () => {
    render(configFor({ protocol: 'duckduckgo', base_url: '', model: '', api_key: null }));
    const fields = searchFields();
    expect(fields).not.toHaveProperty(t.common.baseUrl);
    expect(fields).not.toHaveProperty(t.common.model);
    expect(fields).not.toHaveProperty(t.common.apiKey);
    expect(Object.keys(fields)).toContain(t.common.protocol);
    expect(searchSection().querySelector('.settings-section-badge')).toBeNull();
    expect(searchSection().querySelector('.settings-section-summary')?.textContent).toBe('duckduckgo');
  });

  it('tavily 显示密钥但不显示模型', () => {
    render(configFor({ id: 'search-tavily', protocol: 'tavily', base_url: '', model: '', has_api_key: true }));
    const fields = searchFields();
    expect(fields[t.common.baseUrl]).toBe('');
    expect(fields).not.toHaveProperty(t.common.model);
    expect(fields).toHaveProperty(t.common.apiKey);
    expect(searchSection().querySelector('.settings-section-summary')?.textContent).toBe('tavily');
    expect(searchSection().querySelector('.settings-section-badge')?.textContent).toBe(t.settings.keySet);
  });

  it('grok_search 三个字段都在，摘要带上模型名', () => {
    render(
      configFor({
        id: 'search-grok',
        protocol: 'grok_search',
        base_url: 'https://grok.draem.me',
        model: 'grok-build-0.1',
        has_api_key: true
      })
    );
    const fields = searchFields();
    expect(fields[t.common.baseUrl]).toBe('https://grok.draem.me');
    expect(fields[t.common.model]).toBe('grok-build-0.1');
    expect(fields).toHaveProperty(t.common.apiKey);
    expect(searchSection().querySelector('.settings-section-summary')?.textContent).toBe(
      'grok_search · grok-build-0.1'
    );
  });
});
