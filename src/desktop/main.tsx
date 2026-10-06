import React from 'react';
import { createRoot } from 'react-dom/client';
import { DesktopApp } from './shell';
import { applyTheme, initialTheme } from './prefs';
import '../styles.css';
import './styles.css';

// The window chrome differs per platform and the layout has to match it.
// tauri.conf.json asks for titleBarStyle "Overlay", but that key is
// cfg(target_os = "macos") in tauri-runtime-wry: Windows and Linux keep their
// native title bar, so the space the layout reserves for the traffic lights is
// dead there. Set before the first render so neither the chrome padding nor
// theme flashes.
applyTheme(initialTheme());

if (!navigator.userAgent.includes('Mac')) {
  document.documentElement.classList.add('is-native-chrome');
}

createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>
);
