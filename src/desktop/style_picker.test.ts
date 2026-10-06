import { describe, expect, it } from 'vitest';
import { desktopCopy } from './copy';
import { BACKGROUND_PRESETS, composeRules, FONT_CHOICES, normalizeHex, splitRules, styleLines } from './style_picker';

const zh = desktopCopy.zh.style;
const en = desktopCopy.en.style;

describe('style constraints', () => {
  it('normalises hex colours and rejects anything else', () => {
    expect(normalizeHex('#ffffff')).toBe('#FFFFFF');
    expect(normalizeHex('1f2a44')).toBe('#1F2A44');
    expect(normalizeHex('#abc')).toBe('#AABBCC');
    expect(normalizeHex('#12345')).toBeNull();
    expect(normalizeHex('white')).toBeNull();
    expect(normalizeHex('')).toBeNull();
    for (const preset of BACKGROUND_PRESETS) expect(normalizeHex(preset)).toBe(preset);
  });

  it('adds nothing when nothing was chosen', () => {
    expect(styleLines({ font: '', background: '' }, zh)).toEqual([]);
    expect(composeRules('', { font: '', background: '' }, zh)).toBeNull();
    expect(composeRules('  ', { font: '', background: 'not-a-colour' }, zh)).toBeNull();
  });

  it('appends font and background sentences after the free-text rules', () => {
    const rules = composeRules('禁止出现公式', { font: FONT_CHOICES[0], background: '#eaf2fb' }, zh);
    expect(rules).toBe(`禁止出现公式\n${zh.fontLine(FONT_CHOICES[0])}\n${zh.backgroundLine('#EAF2FB')}`);
    expect(composeRules('', { font: 'Arial', background: '' }, en)).toBe(en.fontLine('Arial'));
  });
});

describe('splitRules', () => {
  it('把选项生成的句子还原回字体与背景色，重发时不会重复追加', () => {
    const rules = composeRules('禁止出现公式', { font: '微软雅黑（Microsoft YaHei）', background: '#FFFFFF' }, zh)!;
    const restored = splitRules(rules, zh);
    expect(restored).toEqual({
      custom: '禁止出现公式',
      choice: { font: '微软雅黑（Microsoft YaHei）', background: '#FFFFFF' }
    });
    expect(composeRules(restored.custom, restored.choice, zh)).toBe(rules);
  });

  it('只剩约束句子时自由文本清空', () => {
    const rules = composeRules('', { font: 'Arial', background: '#eaf2fb' }, zh)!;
    expect(splitRules(rules, zh)).toEqual({
      custom: '',
      choice: { font: 'Arial', background: '#EAF2FB' }
    });
  });

  it('用户自己写的相近句子留在自由文本里', () => {
    const text = '全部文字统一使用字体「不存在的字体」。\n整体背景色使用 #FFF。\n禁止出现公式';
    expect(splitRules(text, zh)).toEqual({ custom: text, choice: { font: '', background: '' } });
  });

  it('同一个选项重复出现一并收走，另一种字体当作自由文本留下', () => {
    const text = [zh.fontLine('Arial'), zh.fontLine('Arial'), zh.fontLine('Inter'), zh.backgroundLine('#FFFFFF')].join('\n');
    expect(splitRules(text, zh)).toEqual({
      custom: zh.fontLine('Inter'),
      choice: { font: 'Arial', background: '#FFFFFF' }
    });
  });

  it('只认当前语言模板写下的句子', () => {
    const rules = composeRules('no formulas', { font: 'Inter', background: '#1F2A44' }, en)!;
    expect(splitRules(rules, zh)).toEqual({ custom: rules, choice: { font: '', background: '' } });
    expect(splitRules(rules, en)).toEqual({
      custom: 'no formulas',
      choice: { font: 'Inter', background: '#1F2A44' }
    });
  });

  it('空文本还原成未选择', () => {
    expect(splitRules('', zh)).toEqual({ custom: '', choice: { font: '', background: '' } });
  });
});
