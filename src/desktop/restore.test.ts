// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';

// 表单还原要走真实副本（含字体/背景色句子），shell 会顺带 import 一堆 Tauri API，
// 这里只要它们不抛错。
vi.mock('../api', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../api')>()),
  desktopAvailable: () => true
}));

import { desktopCopy } from './copy';
import { defaultFigureForm, defaultSlideForm } from './forms';
import { figureFormFrom, slideFormFrom } from './shell';

const zh = desktopCopy.zh.style;

describe('历史任务还原成表单', () => {
  it('约束里的字体与背景色回到选项，而不是留在文本里', () => {
    const payload = {
      figure_title: 'grok bot,muse,dots对比',
      section_description: '对比 Grok bot 和 Muse 还有 dots',
      template_ids: ['tpl-1'],
      aspect_ratio: 'inherit',
      layout_fidelity: 'balanced',
      style_strength: 'high',
      custom_prompt: '禁止出现公式\n全部文字统一使用字体「微软雅黑（Microsoft YaHei）」。\n整体背景色使用 #FFFFFF。'
    };
    const form = figureFormFrom(payload, defaultFigureForm, zh);
    expect(form.title).toBe('grok bot,muse,dots对比');
    expect(form.custom).toBe('禁止出现公式');
    expect(form.style).toEqual({ font: '微软雅黑（Microsoft YaHei）', background: '#FFFFFF' });
  });

  it('幻灯片同样还原，并且没有约束时保持未选', () => {
    const restored = slideFormFrom(
      { material_text: '资料', page_count: 3, custom_prompt: '全部文字统一使用字体「Arial」。' },
      defaultSlideForm,
      zh
    );
    expect(restored.custom).toBe('');
    expect(restored.style).toEqual({ font: 'Arial', background: '' });

    const plain = slideFormFrom({ material_text: '只有资料' }, defaultSlideForm, zh);
    expect(plain.custom).toBe('');
    expect(plain.style).toEqual({ font: '', background: '' });
  });

  it('没有 payload 或字段缺失时退回基准表单', () => {
    expect(figureFormFrom(null, defaultFigureForm, zh)).toEqual(defaultFigureForm);
    const partial = figureFormFrom({ figure_title: '标题' }, defaultFigureForm, zh);
    expect(partial.title).toBe('标题');
    expect(partial.custom).toBe('');
    expect(partial.style).toEqual({ font: '', background: '' });
    expect(partial.layoutFidelity).toBe(defaultFigureForm.layoutFidelity);
  });
});
