// Font and background-colour constraints for the generation forms. The two
// choices are UI state only; `styleLines` turns them into sentences that are
// appended to the free-text rules (`custom_prompt`) at submission, so the
// backend contract is unchanged and nothing is sent when nothing is chosen.

import { useEffect, useRef, useState } from 'react';
import type { DesktopCopy } from './copy';

type StyleCopy = DesktopCopy['style'];

export interface StyleChoice {
  /** Display name inserted verbatim into the prompt; '' means not specified. */
  font: string;
  /** `#RRGGBB`, or '' when not specified. */
  background: string;
}

export const defaultStyleChoice: StyleChoice = { font: '', background: '' };

/** Names an image model knows; both scripts are given for CJK families. */
export const FONT_CHOICES: string[] = [
  '思源黑体（Source Han Sans）',
  '思源宋体（Source Han Serif）',
  '微软雅黑（Microsoft YaHei）',
  '苹方（PingFang SC）',
  '黑体（SimHei）',
  '宋体（SimSun）',
  '楷体（KaiTi）',
  'Arial',
  'Helvetica',
  'Inter',
  'Roboto',
  'Open Sans',
  'Lato',
  'Montserrat',
  'Source Sans Pro',
  'Calibri',
  'Cambria',
  'Times New Roman',
  'Georgia',
  'Garamond',
  'Palatino',
  'Futura',
  'Courier New'
];

export const BACKGROUND_PRESETS: string[] = [
  '#FFFFFF', '#F7F7F5', '#F2F4F7', '#EAF2FB', '#FFF8E1', '#EAF7EE', '#F1ECFA', '#FDECEC', '#1F2A44', '#000000'
];

const HEX = /^#[0-9a-fA-F]{6}$/;

export function normalizeHex(value: string): string | null {
  const trimmed = value.trim();
  const withHash = trimmed.startsWith('#') ? trimmed : `#${trimmed}`;
  if (/^#[0-9a-fA-F]{3}$/.test(withHash)) {
    const [r, g, b] = withHash.slice(1).split('');
    return `#${r}${r}${g}${g}${b}${b}`.toUpperCase();
  }
  return HEX.test(withHash) ? withHash.toUpperCase() : null;
}

/** Sentences to append to the rules text; empty when nothing was chosen. */
export function styleLines(choice: StyleChoice, t: StyleCopy): string[] {
  const lines: string[] = [];
  if (choice.font) lines.push(t.fontLine(choice.font));
  const hex = normalizeHex(choice.background);
  if (hex) lines.push(t.backgroundLine(hex));
  return lines;
}

/** Free-text rules plus the style sentences, or `null` when both are empty. */
export function composeRules(custom: string, choice: StyleChoice, t: StyleCopy): string | null {
  const parts = [custom.trim(), ...styleLines(choice, t)].filter(Boolean);
  return parts.length > 0 ? parts.join('\n') : null;
}

/**
 * `composeRules` 的逆：把存下来的约束文本拆回「自由文本 + 结构化选项」。
 *
 * 只认模板原样生成的句子，认不出的行原封不动留在自由文本里；同一个字体/颜色
 * 重复出现时一并收走（旧记录里可能叠了两遍），另一套字体/颜色的句子则当用户
 * 自己写的约束留下。
 */
export function splitRules(rules: string, t: StyleCopy): { custom: string; choice: StyleChoice } {
  const choice: StyleChoice = { ...defaultStyleChoice };
  const keep: string[] = [];
  for (const line of rules.split('\n')) {
    const sentence = line.trim();
    const font = FONT_CHOICES.find((candidate) => sentence === t.fontLine(candidate));
    if (font && (!choice.font || choice.font === font)) {
      choice.font = font;
      continue;
    }
    const hex = normalizeHex(sentence.match(/#[0-9a-fA-F]{3,6}/)?.[0] ?? '');
    if (hex && sentence === t.backgroundLine(hex) && (!choice.background || choice.background === hex)) {
      choice.background = hex;
      continue;
    }
    keep.push(line);
  }
  return { custom: keep.join('\n').trim(), choice };
}

interface EyeDropperLike {
  open: () => Promise<{ sRGBHex: string }>;
}

function eyeDropper(): EyeDropperLike | null {
  const ctor = (globalThis as { EyeDropper?: new () => EyeDropperLike }).EyeDropper;
  return ctor ? new ctor() : null;
}

export function StyleConstraints({ t, value, onChange }: { t: StyleCopy; value: StyleChoice; onChange: (next: StyleChoice) => void }) {
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(value.background);
  const [invalid, setInvalid] = useState(false);
  const wrap = useRef<HTMLDivElement | null>(null);
  const native = useRef<HTMLInputElement | null>(null);

  useEffect(() => setDraft(value.background), [value.background]);
  useEffect(() => {
    if (!open) return;
    const close = (event: PointerEvent) => {
      if (wrap.current && !wrap.current.contains(event.target as Node)) setOpen(false);
    };
    const escape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', escape);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', escape);
    };
  }, [open]);

  const setBackground = (hex: string) => {
    onChange({ ...value, background: hex });
    setInvalid(false);
  };
  const commitDraft = () => {
    if (draft.trim() === '') {
      setBackground('');
      return;
    }
    const hex = normalizeHex(draft);
    if (hex) setBackground(hex);
    else setInvalid(true);
  };
  const pick = async () => {
    const dropper = eyeDropper();
    if (!dropper) {
      // WebKit has no EyeDropper API; the OS colour panel (which includes a
      // magnifier on macOS) is the closest equivalent.
      native.current?.click();
      return;
    }
    try {
      const result = await dropper.open();
      setBackground(result.sRGBHex.toUpperCase());
    } catch {
      // Cancelled by the user.
    }
  };

  return (
    <div className="dp-style" ref={wrap}>
      <select className="dp-mini" aria-label={t.font} value={value.font} onChange={(event) => onChange({ ...value, font: event.target.value })}>
        <option value="">{t.fontNone}</option>
        {FONT_CHOICES.map((font) => (
          <option key={font} value={font}>
            {font}
          </option>
        ))}
      </select>
      <button
        type="button"
        className={`dp-swatch-btn${value.background ? '' : ' empty'}`}
        title={t.background}
        aria-label={t.background}
        aria-expanded={open}
        onClick={() => setOpen((current) => !current)}
      >
        <span className="dp-swatch" style={value.background ? { background: value.background } : undefined} />
        <span className="dp-swatch-text">{value.background || t.background}</span>
      </button>
      <input
        ref={native}
        type="color"
        className="dp-native-color"
        aria-label={t.systemPicker}
        tabIndex={-1}
        value={normalizeHex(value.background) ?? '#FFFFFF'}
        onChange={(event) => setBackground(event.target.value.toUpperCase())}
      />
      {open && (
        <div className="dp-pop dp-style-pop" role="dialog" aria-label={t.background}>
          <h3>{t.palette}</h3>
          <div className="dp-swatch-grid">
            {BACKGROUND_PRESETS.map((hex) => (
              <button
                key={hex}
                type="button"
                className={`dp-swatch-cell${value.background === hex ? ' active' : ''}`}
                style={{ background: hex }}
                title={hex}
                aria-label={hex}
                onClick={() => setBackground(hex)}
              />
            ))}
          </div>
          <div className="dp-style-row">
            <input
              className={`dp-mini dp-hex${invalid ? ' invalid' : ''}`}
              placeholder={t.hex}
              value={draft}
              spellCheck={false}
              onChange={(event) => {
                setDraft(event.target.value);
                setInvalid(false);
              }}
              onBlur={commitDraft}
              onKeyDown={(event) => {
                if (event.key === 'Enter') commitDraft();
              }}
            />
            <button type="button" className="dp-mini dp-style-action" title={t.eyedropperHint} onClick={() => void pick()}>
              {t.eyedropper}
            </button>
            <button type="button" className="dp-mini dp-style-action" onClick={() => native.current?.click()}>
              {t.systemPicker}
            </button>
            <button type="button" className="dp-mini dp-style-action" disabled={!value.background} onClick={() => setBackground('')}>
              {t.clear}
            </button>
          </div>
          {invalid && <p className="dp-style-error">{t.invalidHex}</p>}
        </div>
      )}
    </div>
  );
}
