import { describe, expect, it } from 'vitest';
import { activeSearchProfile, searchNeedsNoSetup, searchUsesModel, withSearchProtocol } from './app';
import type { AppConfig, ModelProfile } from './types';

function searchProfile(values: Partial<ModelProfile>): ModelProfile {
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

function config(profiles: ModelProfile[], active: string): AppConfig {
  return {
    version: 1,
    active_design_profile: 'design-default',
    active_implement_profile: 'implement-default',
    active_search_profile: active,
    model_profiles: [
      { ...searchProfile({ id: 'design-default', role: 'design', protocol: 'openai_chat' }) },
      ...profiles
    ]
  };
}

describe('search protocol profiles', () => {
  it('只有对话式联网搜索需要模型', () => {
    expect(searchUsesModel('duckduckgo')).toBe(false);
    expect(searchUsesModel('tavily')).toBe(false);
    expect(searchUsesModel('grok_search')).toBe(true);
  });

  it('免密钥那个协议一个字段都不用填：地址与密钥都不显示', () => {
    expect(searchNeedsNoSetup('duckduckgo')).toBe(true);
    expect(searchNeedsNoSetup('tavily')).toBe(false);
    expect(searchNeedsNoSetup('grok_search')).toBe(false);
  });

  it('取的是 active_search_profile 指向的那份，而不是列表里第一份', () => {
    const grok = searchProfile({
      id: 'search-grok',
      protocol: 'grok_search',
      base_url: 'https://grok.draem.me',
      model: 'grok-build-0.1',
      has_api_key: true
    });
    const duck = searchProfile({});
    expect(activeSearchProfile(config([duck, grok], 'search-grok'))).toBe(grok);
    expect(activeSearchProfile(config([duck, grok], 'search-missing'))).toBe(duck);
  });

  it('切到已有协议只是把它设为当前，不动任何值', () => {
    const grok = searchProfile({
      id: 'search-grok',
      protocol: 'grok_search',
      base_url: 'https://grok.draem.me',
      model: 'grok-build-0.1'
    });
    const next = withSearchProtocol(config([searchProfile({}), grok], 'search-default'), 'grok_search');
    expect(next.active_search_profile).toBe('search-grok');
    expect(next.model_profiles).toHaveLength(3);
    expect(activeSearchProfile(next).base_url).toBe('https://grok.draem.me');
  });

  it('切到没有的协议补一份干净的档案，不会带上别的协议的值', () => {
    const current = searchProfile({
      base_url: 'https://grok.draem.me',
      model: 'grok-build-0.1',
      api_key: 'xai-secret',
      has_api_key: true
    });
    const next = withSearchProtocol(config([current], 'search-default'), 'tavily');
    const created = activeSearchProfile(next);
    expect(created.protocol).toBe('tavily');
    expect(created.id).toBe('search-tavily');
    expect(created.base_url).toBe('');
    expect(created.model).toBe('');
    expect(created.api_key ?? null).toBeNull();
    expect(created.has_api_key).toBe(false);
    // 原来那份原封不动，两份共存。
    expect(next.model_profiles.find((item) => item.id === 'search-default')?.base_url).toBe(
      'https://grok.draem.me'
    );
    expect(searchUsesModel(created.protocol)).toBe(false);
  });

  it('id 被占用时换一个可用的 id', () => {
    const taken = searchProfile({ id: 'search-tavily', protocol: 'grok_search', model: 'x' });
    const next = withSearchProtocol(config([taken], 'search-default'), 'tavily');
    expect(activeSearchProfile(next).id).toBe('search-tavily-2');
    expect(new Set(next.model_profiles.map((item) => item.id)).size).toBe(next.model_profiles.length);
  });

  it('重复切同一个协议不会一直加档案', () => {
    const once = withSearchProtocol(config([searchProfile({})], 'search-default'), 'tavily');
    const twice = withSearchProtocol(once, 'tavily');
    const back = withSearchProtocol(twice, 'duckduckgo');
    expect(twice.model_profiles).toHaveLength(once.model_profiles.length);
    expect(activeSearchProfile(back).id).toBe('search-default');
    expect(back.model_profiles).toHaveLength(3);
  });
});
