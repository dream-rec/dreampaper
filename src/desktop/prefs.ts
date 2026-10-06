export type Theme = 'light' | 'dark';

const THEME_KEY = 'dreampaper.theme';
const AUTO_UPDATE_KEY = 'dreampaper.autoUpdate';
const SIMPLE_MODE_KEY = 'dreampaper.simpleMode';
const SIMPLE_MODE_ACK_KEY = 'dreampaper.simpleModeAck';

function read(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    window.localStorage.setItem(key, value);
  } catch {
    return;
  }
}

export function systemTheme(): Theme {
  return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
}

export function themePreference(): Theme | null {
  const value = read(THEME_KEY);
  return value === 'light' || value === 'dark' ? value : null;
}

export function initialTheme(): Theme {
  return themePreference() ?? systemTheme();
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

export function saveTheme(theme: Theme): void {
  write(THEME_KEY, theme);
}

export function autoUpdateEnabled(): boolean {
  return read(AUTO_UPDATE_KEY) !== 'false';
}

export function saveAutoUpdate(enabled: boolean): void {
  write(AUTO_UPDATE_KEY, String(enabled));
}

/** Seeds the Simple Mode switch; the live switch lives in React state, not here. */
export function simpleModePreference(): boolean {
  return read(SIMPLE_MODE_KEY) === 'true';
}

export function saveSimpleMode(enabled: boolean): void {
  write(SIMPLE_MODE_KEY, String(enabled));
}

/** First time Simple Mode is turned on, the notice stays until this is set. */
export function simpleModeAcknowledged(): boolean {
  return read(SIMPLE_MODE_ACK_KEY) === 'true';
}

export function acknowledgeSimpleMode(): void {
  write(SIMPLE_MODE_ACK_KEY, 'true');
}
